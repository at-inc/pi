//! Port of `packages/schema/src/resolver.ts`.
//! PORT-STATUS: ported
//!
//! Reference resolution of one compile: schema resources, anchors and JSON Pointers over the root schema and the
//! registered documents. A Resolver lives only while a schema compiles (or while its errors are explained).
//!
//! Registered documents are copied into the schema tree (`Node`) when a reference first loads them. Maps keyed by
//! schema object hold the object too, so its address stays its own (see `SchemaObject::id`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use pi_host::{PiError, PiResult};

use crate::types::{CompileOptions, Draft, Node, SchemaObject, UnresolvedRefs};

/// A schema and the base URI its references resolve against.
#[derive(Clone, Debug)]
pub struct Resolved {
	pub node: Node,
	pub base: String,
}

/// The base URI of a root schema without `$id`; relative references resolve against it.
pub const DEFAULT_BASE: &str = "https://json-schema.invalid/root.json";

/// Resolves references in the root schema and the registered documents. The schema is indexed on the first
/// reference, so a schema without references is never walked for identifiers.
pub struct Resolver {
	pub draft: Draft,
	pub root_base: String,
	/// Schema object to the base URI it was indexed under.
	base_of: RefCell<HashMap<usize, (SchemaObject, String)>>,
	/// URI (with a `#name` fragment) of every `$dynamicAnchor`, to its schema.
	dynamic_anchors: RefCell<HashMap<String, SchemaObject>>,
	root: Node,
	options: CompileOptions,
	/// Document URI to the key of its schema in `options.schemas`.
	registry: RefCell<HashMap<String, String>>,
	reject_unresolved: bool,
	/// The references that named nothing and resolved to `false` (with `unresolvedRefs: "reject"`).
	unresolved: RefCell<HashSet<String>>,
	resources: RefCell<HashMap<String, Node>>,
	anchors: RefCell<HashMap<String, SchemaObject>>,
	indexed: Cell<bool>,
}

impl Resolver {
	/// `new Resolver(root, options)`.
	pub fn new(root: Node, options: CompileOptions) -> Resolver {
		let draft = if options.draft == Some(Draft::V7) {
			Draft::V7
		} else {
			Draft::V2020_12
		};
		let reject_unresolved = options.unresolved_refs == Some(UnresolvedRefs::Reject);
		let mut resolver = Resolver {
			draft,
			root_base: DEFAULT_BASE.to_string(),
			base_of: RefCell::new(HashMap::new()),
			dynamic_anchors: RefCell::new(HashMap::new()),
			root,
			options,
			registry: RefCell::new(HashMap::new()),
			reject_unresolved,
			unresolved: RefCell::new(HashSet::new()),
			resources: RefCell::new(HashMap::new()),
			anchors: RefCell::new(HashMap::new()),
			indexed: Cell::new(false),
		};
		if let Some(root) = resolver.root.as_object() {
			resolver.root_base = resolver.base_for(root, DEFAULT_BASE);
		}
		resolver
	}

	/// The base URI `node` was indexed under (`baseOf.get(node)`).
	pub fn base_of(&self, node: &SchemaObject) -> Option<String> {
		self.base_of.borrow().get(&node.id()).map(|(_, base)| base.clone())
	}

	/// The schema of a `$dynamicAnchor` by its URI with a `#name` fragment (`dynamicAnchors.get(key)`).
	pub fn dynamic_anchor(&self, key: &str) -> Option<SchemaObject> {
		self.dynamic_anchors.borrow().get(key).cloned()
	}

	/// The base URI of `node`, whose parent has `base`: its `$id` changes it, unless draft 7 makes the `$id` inert. Once
	/// the schema is indexed, the indexed base (a resolved reference target already has its own).
	pub fn base_for(&self, node: &SchemaObject, base: &str) -> String {
		if let Some(indexed) = self.base_of(node) {
			return indexed;
		}
		let Some(id) = node.str("$id") else {
			return base.to_string();
		};
		if self.draft == Draft::V7 && (node.str("$ref").is_some() || id.starts_with('#')) {
			return base.to_string();
		}
		document_of(&resolve_uri(id, base)).to_string()
	}

	/// Indexes the root schema and the registered documents' URIs (once).
	pub fn ensure_indexed(&self) {
		if self.indexed.get() {
			return;
		}
		self.indexed.set(true);
		if let Some(schemas) = &self.options.schemas {
			let mut registry = self.registry.borrow_mut();
			for uri in schemas.keys() {
				registry.insert(document_of(&resolve_uri(uri, DEFAULT_BASE)).to_string(), uri.clone());
			}
		}
		self.resources
			.borrow_mut()
			.insert(DEFAULT_BASE.to_string(), self.root.clone());
		self.index(&self.root.clone(), DEFAULT_BASE);
	}

	/// The schema a reference names, with the base URI it compiles under. Throws if it names nothing, or, with
	/// `unresolvedRefs: "reject"`, resolves to `false`, the schema no value passes.
	pub fn resolve(&self, reference: &str, base: &str) -> PiResult<Resolved> {
		self.ensure_indexed();
		if !self.reject_unresolved {
			return self.find(reference, base);
		}
		match self.find(reference, base) {
			Ok(resolved) => Ok(resolved),
			Err(_) => {
				self.unresolved.borrow_mut().insert(resolve_uri(reference, base));
				Ok(Resolved {
					node: Node::Bool(false),
					base: base.to_string(),
				})
			}
		}
	}

	/// Whether `reference`, from `base`, named nothing and resolved to `false`.
	pub fn is_unresolved(&self, reference: &str, base: &str) -> bool {
		self.unresolved.borrow().contains(&resolve_uri(reference, base))
	}

	fn find(&self, reference: &str, base: &str) -> PiResult<Resolved> {
		let uri = resolve_uri(reference, base);
		let doc = document_of(&uri).to_string();
		let fragment = if uri.len() > doc.len() {
			decode_uri_component(&uri[doc.len() + 1..])?
		} else {
			String::new()
		};
		let existing = self.resources.borrow().get(&doc).cloned();
		let root = match existing {
			Some(root) => Some(root),
			None => self.load(&doc),
		};
		let Some(root) = root else {
			return Err(PiError::new(format!(
				"Unresolved $ref {reference} (no schema registered for {doc})"
			)));
		};
		if fragment.is_empty() {
			return Ok(Resolved { node: root, base: doc });
		}
		if fragment.starts_with('/') {
			return self.pointer(&root, &doc, &fragment);
		}
		let target = self.anchors.borrow().get(&format!("{doc}#{fragment}")).cloned();
		let Some(target) = target else {
			return Err(PiError::new(format!(
				"Unresolved $ref {reference} (no anchor {fragment} in {doc})"
			)));
		};
		let base = self.base_of(&target).unwrap_or(doc);
		Ok(Resolved {
			node: Node::Object(target),
			base,
		})
	}

	/// Whether `node` is the root of the schema resource at `base`.
	pub fn is_resource(&self, node: &SchemaObject, base: &str) -> bool {
		self.ensure_indexed();
		matches!(self.resources.borrow().get(base), Some(Node::Object(resource)) if resource.same(node))
	}

	// Walks only keywords that hold subschemas: an `$id` inside `enum`, `const` or an unknown keyword names nothing.
	fn index(&self, node: &Node, base: &str) {
		let Some(node) = node.as_object() else {
			return;
		};
		if self.base_of.borrow().contains_key(&node.id()) {
			return;
		}
		let mut b = base.to_string();
		// In draft 7 a `$ref` makes every sibling keyword, `$id` included, inert.
		let ref_only = self.draft == Draft::V7 && node.str("$ref").is_some();
		if !ref_only && let Some(id) = node.str("$id") {
			if self.draft == Draft::V7 && id.starts_with('#') {
				self.anchors
					.borrow_mut()
					.insert(format!("{}{id}", document_of(&b)), node.clone());
			} else {
				let uri = resolve_uri(id, &b);
				b = document_of(&uri).to_string();
				self.resources
					.borrow_mut()
					.insert(b.clone(), Node::Object(node.clone()));
				if uri.len() > b.len() + 1 {
					self.anchors.borrow_mut().insert(uri, node.clone());
				}
			}
		}
		self.base_of.borrow_mut().insert(node.id(), (node.clone(), b.clone()));
		if ref_only {
			return;
		}
		if self.draft != Draft::V7 {
			if let Some(anchor) = node.str("$anchor") {
				self.anchors.borrow_mut().insert(format!("{b}#{anchor}"), node.clone());
			}
			if let Some(anchor) = node.str("$dynamicAnchor") {
				self.anchors.borrow_mut().insert(format!("{b}#{anchor}"), node.clone());
				self.dynamic_anchors
					.borrow_mut()
					.insert(format!("{b}#{anchor}"), node.clone());
			}
		}
		for key in SCHEMA_KEYWORDS {
			if let Some(child) = node.get(key) {
				self.index(child, &b);
			}
		}
		for key in MAP_KEYWORDS {
			if let Some(map) = node.object(key) {
				for (_, child) in map.entries() {
					self.index(child, &b);
				}
			}
		}
		for key in ARRAY_KEYWORDS {
			if let Some(list) = node.array(key) {
				for child in list {
					self.index(child, &b);
				}
			}
		}
	}

	fn load(&self, doc: &str) -> Option<Node> {
		let key = self.registry.borrow().get(doc).cloned()?;
		let schema = Node::from_value(self.options.schemas.as_ref()?.get(&key)?);
		self.resources.borrow_mut().insert(doc.to_string(), schema.clone());
		self.index(&schema, doc);
		Some(schema)
	}

	fn pointer(&self, root: &Node, base: &str, fragment: &str) -> PiResult<Resolved> {
		let mut node = root.clone();
		let mut b = base.to_string();
		for raw in fragment.split('/').skip(1) {
			let key = raw.replace("~1", "/").replace("~0", "~");
			let next = own_property(&node, &key);
			let Some(next) = next else {
				return Err(PiError::new(format!("Unresolved $ref #{fragment} in {base}")));
			};
			node = next;
			if let Some(object) = node.as_object()
				&& let Some(indexed) = self.base_of(object)
			{
				b = indexed;
			}
		}
		Ok(Resolved { node, base: b })
	}
}

// `Object.hasOwn(node, key) ? node[key] : undefined` for an object or an array (whose own properties are its
// indexes and `length`).
fn own_property(node: &Node, key: &str) -> Option<Node> {
	match node {
		Node::Object(object) => object.get(key).cloned(),
		Node::Array(items) => {
			if key == "length" {
				return Some(Node::Number(serde_json::Number::from(items.len())));
			}
			let canonical = key == "0" || (!key.starts_with('0') && key.chars().all(|c| c.is_ascii_digit()));
			if !canonical || key.is_empty() {
				return None;
			}
			items.get(key.parse::<usize>().ok()?).cloned()
		}
		_ => None,
	}
}

const SCHEMA_KEYWORDS: [&str; 12] = [
	"additionalProperties",
	"propertyNames",
	"contains",
	"not",
	"if",
	"then",
	"else",
	"unevaluatedProperties",
	"unevaluatedItems",
	"additionalItems",
	"contentSchema",
	"items",
];

const MAP_KEYWORDS: [&str; 6] = [
	"properties",
	"patternProperties",
	"$defs",
	"definitions",
	"dependentSchemas",
	"dependencies",
];

const ARRAY_KEYWORDS: [&str; 5] = ["allOf", "anyOf", "oneOf", "prefixItems", "items"];

/// Whether `value` is a schema object (not `true`/`false`, an array or a primitive).
pub fn is_schema_object(value: &Node) -> bool {
	value.as_object().is_some()
}

/// `ref` resolved against `base`. URNs and other bases a URL parser rejects take fragment-only references.
pub fn resolve_uri(reference: &str, base: &str) -> String {
	match url::Url::parse(base).and_then(|base| base.join(reference)) {
		Ok(url) => url.as_str().to_string(),
		Err(_) => {
			if reference.starts_with('#') {
				format!("{}{reference}", document_of(base))
			} else {
				reference.to_string()
			}
		}
	}
}

/// `uri` without its fragment.
pub fn document_of(uri: &str) -> &str {
	match uri.find('#') {
		Some(hash) => &uri[..hash],
		None => uri,
	}
}

/// `decodeURIComponent(value)`: throws a URIError for a malformed escape.
fn decode_uri_component(value: &str) -> PiResult<String> {
	let malformed = || PiError::named("URIError", "URI malformed");
	let bytes = value.as_bytes();
	let mut out = Vec::with_capacity(bytes.len());
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] != b'%' {
			out.push(bytes[i]);
			i += 1;
			continue;
		}
		let hex = value.get(i + 1..i + 3).ok_or_else(malformed)?;
		if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
			return Err(malformed());
		}
		out.push(u8::from_str_radix(hex, 16).map_err(|_| malformed())?);
		i += 3;
	}
	String::from_utf8(out).map_err(|_| malformed())
}
