//! Port of `packages/schema/src/validator.ts`.
//! PORT-STATUS: ported
//!
//! compile(): a schema to a Validator, whose `check` is the fast boolean path and whose `errors` explains failures.
//!
//! Rust port: the two TypeScript overloads of compile() are two functions. `compile()` returns a `Validator` (it ignores
//! `coerce`), `compile_coercing()` is the `coerce: true` overload and returns a `CoercingValidator`. The type parameter
//! `T` (the static type of valid values) has no runtime effect and is dropped. errors(), coerce() and parse() return
//! `PiResult` where the TypeScript can throw (an explainer or coercer whose schema fails to compile, an invalid
//! pattern). Where the TypeScript check throws (a reference that recurses without end, a subschema that failed to
//! compile; see `checks.rs`), errors() and parse() throw the same error and check() returns false (the raw `check`
//! function may return either).

use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

use pi_host::PiResult;
use serde_json::Value;

use crate::checks::catching;
use crate::coerce::Coercer;
use crate::compiler::{Compiler, is_restart};
use crate::explain::Explainer;
use crate::types::{Check, CompileOptions, JsonSchema, Node, ParseResult, ValidationError};

/// Compiles `schema` into a Validator. Throws if the schema is malformed or a `$ref` names nothing (register other
/// documents with `options.schemas`). The schema is read, never modified. With `coerce: true` (compile_coercing()) the
/// validator also coerces and parses loosely typed input.
pub fn compile(schema: &JsonSchema, options: &CompileOptions) -> PiResult<Validator> {
	let schema = Node::from_value(schema);
	let check = compile_check(&schema, options)?;
	Ok(Validator::new(schema, options.clone(), check))
}

/// `compile(schema, { ...options, coerce: true })`: a validator that also coerces and parses loosely typed input.
pub fn compile_coercing(schema: &JsonSchema, options: &CompileOptions) -> PiResult<CoercingValidator> {
	let options = CompileOptions {
		coerce: Some(true),
		..options.clone()
	};
	let schema = Node::from_value(schema);
	let check = compile_check(&schema, &options)?;
	Ok(CoercingValidator::new(schema, options, check))
}

/// A compiled schema.
pub struct Validator {
	/// Whether `value` is valid. A plain function: it may be passed around without its validator.
	pub check: Check,
	schema: Node,
	options: CompileOptions,
	explainer: RefCell<Option<Rc<Explainer>>>,
}

impl Validator {
	/// `new Validator(schema, options, check)`.
	pub fn new(schema: Node, options: CompileOptions, check: Check) -> Validator {
		Validator {
			check,
			schema,
			options,
			explainer: RefCell::new(None),
		}
	}

	/// Whether `value` is valid (`validator.check(value)`).
	pub fn check(&self, value: &Value) -> bool {
		catching(|| (self.check)(value)).unwrap_or(false)
	}

	/// Why `value` is invalid: one error per failing keyword, `[]` when it is valid. Built on the first failure and
	/// kept, so a validator whose values always pass never pays for it.
	pub fn errors(&self, value: &Value) -> PiResult<Vec<ValidationError>> {
		if catching(|| (self.check)(value))? {
			return Ok(Vec::new());
		}
		let explainer = self.explainer.borrow().clone();
		let explainer = match explainer {
			Some(explainer) => explainer,
			None => {
				let explainer = Rc::new(Explainer::new(
					compiler(&self.schema, &self.options)?,
					self.schema.clone(),
				)?);
				*self.explainer.borrow_mut() = Some(explainer.clone());
				explainer
			}
		};
		catching(|| explainer.errors(value))?
	}
}

/// A validator compiled with `coerce: true`: it also coerces loosely typed input towards the schema.
pub struct CoercingValidator {
	validator: Validator,
	coercer: RefCell<Option<Rc<Coercer>>>,
}

impl Deref for CoercingValidator {
	type Target = Validator;

	fn deref(&self) -> &Validator {
		&self.validator
	}
}

impl CoercingValidator {
	/// `new CoercingValidator(schema, options, check)`.
	pub fn new(schema: Node, options: CompileOptions, check: Check) -> CoercingValidator {
		CoercingValidator {
			validator: Validator::new(schema, options, check),
			coercer: RefCell::new(None),
		}
	}

	/// `value` coerced towards the schema: numeric strings to numbers, `"true"` to `true`, a single value to a one-item
	/// array, an optional `null` dropped, the first union branch the value converts to (the README lists every rule).
	/// Only the changed paths are copied; `value` comes back itself (`Cow::Borrowed`) when nothing changes, and is never
	/// modified. Values that cannot be coerced stay as they are. The rules are built on the first call.
	pub fn coerce<'a>(&self, value: &'a Value) -> PiResult<Cow<'a, Value>> {
		let coercer = self.coercer.borrow().clone();
		let coercer = match coercer {
			Some(coercer) => coercer,
			None => {
				let v = &self.validator;
				let coercer = Rc::new(Coercer::new(compiler(&v.schema, &v.options)?, v.schema.clone()));
				*self.coercer.borrow_mut() = Some(coercer.clone());
				coercer
			}
		};
		coercer.coerce(value)
	}

	/// Coerces `value`, then checks it: the coerced value when it is valid, else its errors.
	pub fn parse(&self, value: &Value) -> PiResult<ParseResult> {
		let coerced = self.coerce(value)?;
		if catching(|| (self.check)(&coerced))? {
			return Ok(ParseResult::Ok(coerced.into_owned()));
		}
		Ok(ParseResult::Err(self.errors(&coerced)?))
	}
}

fn compile_check(schema: &Node, options: &CompileOptions) -> PiResult<Check> {
	compiler(schema, options)?.root()
}

// Most schemas compile without tracking the dynamic scope; one with a bookended $dynamicRef compiles again with it.
fn compiler(schema: &Node, options: &CompileOptions) -> PiResult<Compiler> {
	let plain = Compiler::new(schema.clone(), options, false);
	match plain.root() {
		Ok(_) => Ok(plain),
		Err(error) if is_restart(&error) => Ok(Compiler::new(schema.clone(), options, true)),
		Err(error) => Err(error),
	}
}
