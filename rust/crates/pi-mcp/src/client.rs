//! Port of `packages/mcp/src/client.ts`.
//! PORT-STATUS: ported
//!
//! `McpClient` is a cheap handle (`Clone` shares the client). The methods whose TS body runs synchronously up
//! to the first `await` (`connect`, `request`, `notify`, `ping`, `callTool`, `readResource`, `close`) do that
//! prefix when called and return the rest as a future, so a request is assigned its id and dispatched even if
//! the returned future is polled later. The list methods are plain `async fn`s and start when polled.
//! Request timeouts are timer tasks on the host (`pi_host::sleep`), cancelled when the request settles.
//!
//! Listeners that the TS wraps in `try/catch` (notification, close and progress listeners) return
//! `PiResult<()>`; an `Err` is reported through `onError` like a thrown error. Results the server sends are
//! checked like the TS and then deserialized into the protocol types; a value that passes the TS checks but
//! has a field of the wrong type (for example a tool whose `title` is a number) fails with the same
//! "Invalid ..." error as an entry that fails the checks. A progress notification whose fields do not have the
//! `ProgressNotification` types still renews the timeout but is not passed to `onProgress`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use futures::channel::oneshot;
use futures::future::{AbortHandle, abortable};
use indexmap::{IndexMap, IndexSet};
use pi_host::abort::ListenerId;
use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::protocol::content::{CallToolResult, js_string};
use crate::protocol::jsonrpc::{
	JSON_RPC_ERROR_CODES, JsonRpcErrorObject, JsonRpcId, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse,
	McpAbortError, McpConnectionClosedError, McpError, McpTimeoutError, is_json_rpc_id, is_object, js_error_string,
};
use crate::protocol::types::{
	ClientCapabilities, Implementation, InitializeResult, LATEST_PROTOCOL_VERSION, ListResourceTemplatesResult,
	ListResourcesResult, ProgressNotification, ReadResourceResult, Resource, ResourceTemplate, Root,
	SUPPORTED_PROTOCOL_VERSIONS, ServerCapabilities, SupportedProtocolVersion, Tool,
};
use crate::transports::transport::{McpTransport, McpTransportSendOptions, Unsubscribe};

const DEFAULT_REQUEST_TIMEOUT_MS: f64 = 30_000.0;
const MAX_LIST_PAGES: usize = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientState {
	#[serde(rename = "idle")]
	Idle,
	#[serde(rename = "connecting")]
	Connecting,
	#[serde(rename = "connected")]
	Connected,
	#[serde(rename = "closed")]
	Closed,
}

impl ClientState {
	/// The TS string value.
	pub fn as_str(self) -> &'static str {
		match self {
			ClientState::Idle => "idle",
			ClientState::Connecting => "connecting",
			ClientState::Connected => "connected",
			ClientState::Closed => "closed",
		}
	}
}

pub type NotificationListener = Rc<dyn Fn(Option<&Value>) -> PiResult<()>>;
pub type ErrorListener = Rc<dyn Fn(&PiError)>;
pub type CloseListener = Rc<dyn Fn() -> PiResult<()>>;
pub type ProgressListener = Rc<dyn Fn(&ProgressNotification) -> PiResult<()>>;

/// The `{ signal }` context a request handler receives.
#[derive(Clone, Debug)]
pub struct RequestHandlerContext {
	pub signal: AbortSignal,
}

/// Answers a server-to-client request. `null` answers `{}`, like `undefined` in TS.
pub type RequestHandler = Rc<dyn Fn(Option<Value>, RequestHandlerContext) -> LocalBoxFuture<'static, PiResult<Value>>>;

/// `roots?: readonly Root[] | (() => readonly Root[] | Promise<readonly Root[]>)`.
#[derive(Clone)]
pub enum McpRoots {
	List(Vec<Root>),
	Provider(Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<Vec<Root>>>>),
}

/// `interface McpClientOptions extends Implementation`.
#[derive(Clone, Default)]
pub struct McpClientOptions {
	pub name: String,
	pub version: String,
	pub title: Option<String>,
	pub capabilities: Option<ClientCapabilities>,
	pub protocol_version: Option<SupportedProtocolVersion>,
	pub request_timeout_ms: Option<f64>,
	pub roots: Option<McpRoots>,
}

#[derive(Clone, Default)]
pub struct McpRequestOptions {
	pub signal: Option<AbortSignal>,
	pub timeout_ms: Option<f64>,
	pub on_progress: Option<ProgressListener>,
}

struct PendingRequest {
	sender: oneshot::Sender<PiResult<Value>>,
	timeout_ms: f64,
	timer: Option<AbortHandle>,
	signal: Option<AbortSignal>,
	abort_listener: Option<ListenerId>,
	/// Aborted when the request settles, so the transport does not dispatch (or retry) it afterwards.
	send_controller: AbortController,
	cancellable: bool,
	on_progress: Option<ProgressListener>,
	progress_token: Option<JsonRpcId>,
}

fn validate_initialize_result(value: Value) -> PiResult<InitializeResult> {
	let invalid = || {
		McpError::new(
			JSON_RPC_ERROR_CODES::INVALID_REQUEST,
			"Invalid MCP initialize result",
			None,
		)
	};
	let Some(object) = value.as_object() else {
		return Err(invalid());
	};
	let server_info = object.get("serverInfo").and_then(Value::as_object);
	let valid = object.get("protocolVersion").is_some_and(Value::is_string)
		&& object.get("capabilities").is_some_and(is_object)
		&& server_info.is_some_and(|info| {
			info.get("name").is_some_and(Value::is_string) && info.get("version").is_some_and(Value::is_string)
		}) && object.get("instructions").is_none_or(Value::is_string);
	if !valid {
		return Err(invalid());
	}
	serde_json::from_value(value).map_err(|_| invalid())
}

fn invalid(message: impl Into<String>) -> PiError {
	McpError::new(JSON_RPC_ERROR_CODES::INVALID_REQUEST, message, None)
}

/// One page of a paginated list: the items under `key`, each checked by `is_item`.
struct ListPage {
	items: Vec<Map<String, Value>>,
	next_cursor: Option<String>,
}

type IsItem = fn(&Map<String, Value>) -> bool;

fn validate_list_page(method: &str, key: &str, value: Value, is_item: IsItem) -> PiResult<ListPage> {
	let Value::Object(mut object) = value else {
		return Err(invalid(format!("Invalid MCP {method} result")));
	};
	let Some(Value::Array(items)) = object.remove(key) else {
		return Err(invalid(format!("Invalid MCP {method} result")));
	};
	let mut checked = Vec::with_capacity(items.len());
	for item in items {
		match item {
			Value::Object(item) if is_item(&item) => checked.push(item),
			_ => return Err(invalid(format!("Invalid entry in MCP {method} result"))),
		}
	}
	// Some servers end pagination with `null` or `""` instead of omitting the cursor.
	let next_cursor = match object.remove("nextCursor") {
		None | Some(Value::Null) => None,
		Some(Value::String(cursor)) if cursor.is_empty() => None,
		Some(Value::String(cursor)) => Some(cursor),
		Some(_) => return Err(invalid(format!("Invalid MCP {method} cursor"))),
	};
	Ok(ListPage {
		items: checked,
		next_cursor,
	})
}

fn is_tool(tool: &Map<String, Value>) -> bool {
	tool.get("name").is_some_and(Value::is_string) && tool.get("inputSchema").is_some_and(is_object)
}

// `name` is required by the spec, but some servers omit it; the URI stands in.
fn is_resource(resource: &Map<String, Value>) -> bool {
	resource.get("uri").is_some_and(Value::is_string) && resource.get("name").is_none_or(Value::is_string)
}

fn is_resource_template(template: &Map<String, Value>) -> bool {
	template.get("uriTemplate").is_some_and(Value::is_string) && template.get("name").is_none_or(Value::is_string)
}

fn with_name_from(mut item: Map<String, Value>, key: &str) -> Value {
	if item.get("name").is_none_or(Value::is_null) {
		let fallback = item.get(key).cloned().unwrap_or(Value::Null);
		item.insert("name".into(), fallback);
	}
	Value::Object(item)
}

fn items_as<T: for<'de> Deserialize<'de>>(method: &str, items: Vec<Value>) -> PiResult<Vec<T>> {
	items
		.into_iter()
		.map(|item| serde_json::from_value(item).map_err(|_| invalid(format!("Invalid entry in MCP {method} result"))))
		.collect()
}

fn to_resources(items: Vec<Map<String, Value>>) -> PiResult<Vec<Resource>> {
	items_as(
		"resources/list",
		items.into_iter().map(|item| with_name_from(item, "uri")).collect(),
	)
}

fn to_resource_templates(items: Vec<Map<String, Value>>) -> PiResult<Vec<ResourceTemplate>> {
	items_as(
		"resources/templates/list",
		items
			.into_iter()
			.map(|item| with_name_from(item, "uriTemplate"))
			.collect(),
	)
}

fn validate_read_resource_result(value: Value) -> PiResult<ReadResourceResult> {
	let Some(contents) = value
		.get("contents")
		.and_then(Value::as_array)
		.filter(|_| value.is_object())
	else {
		return Err(invalid("Invalid MCP resources/read result"));
	};
	for contents in contents {
		let valid = contents.as_object().is_some_and(|contents| {
			contents.get("uri").is_some_and(Value::is_string)
				&& (contents.get("text").is_some_and(Value::is_string)
					|| contents.get("blob").is_some_and(Value::is_string))
		});
		if !valid {
			return Err(invalid("Invalid contents in MCP resources/read result"));
		}
	}
	serde_json::from_value(value).map_err(|_| invalid("Invalid contents in MCP resources/read result"))
}

/// `content` is required by the spec, but servers that only return `structuredContent` omit it (the SDK defaults it too).
/// `structuredContent` may be any JSON value, as MCP 2026-07-28 allows.
fn validate_call_tool_result(value: Value) -> PiResult<CallToolResult> {
	let invalid = || {
		McpError::new(
			JSON_RPC_ERROR_CODES::INVALID_REQUEST,
			"Invalid MCP tools/call result",
			None,
		)
	};
	let Value::Object(mut object) = value else {
		return Err(invalid());
	};
	match object.get("content") {
		None => {
			object.insert("content".into(), Value::Array(Vec::new()));
		}
		Some(Value::Array(_)) => {}
		Some(_) => return Err(invalid()),
	}
	serde_json::from_value(Value::Object(object)).map_err(|_| invalid())
}

struct State {
	state: ClientState,
	transport: Option<Rc<dyn McpTransport>>,
	next_request_id: f64,
	server_info: Option<Implementation>,
	server_capabilities: Option<ServerCapabilities>,
	instructions: Option<String>,
	protocol_version: Option<String>,
	pending: IndexMap<JsonRpcId, PendingRequest>,
	progress_requests: IndexMap<JsonRpcId, JsonRpcId>,
	incoming: IndexMap<JsonRpcId, AbortController>,
	request_handlers: IndexMap<String, RequestHandler>,
	notification_listeners: IndexMap<String, Vec<(u64, NotificationListener)>>,
	error_listeners: Vec<(u64, ErrorListener)>,
	close_listeners: Vec<(u64, CloseListener)>,
	next_listener_id: u64,
	disposers: Vec<Unsubscribe>,
}

struct Inner {
	options: McpClientOptions,
	state: RefCell<State>,
}

#[derive(Clone)]
pub struct McpClient {
	inner: Rc<Inner>,
}

impl McpClient {
	/// `new McpClient(options)`.
	pub fn new(options: McpClientOptions) -> Self {
		let mut request_handlers: IndexMap<String, RequestHandler> = IndexMap::new();
		request_handlers.insert("ping".into(), Rc::new(|_, _| Box::pin(async { Ok(json!({})) })));
		if let Some(roots) = options.roots.clone() {
			request_handlers.insert(
				"roots/list".into(),
				Rc::new(move |_, _| {
					let roots = roots.clone();
					Box::pin(async move {
						let roots = match roots {
							McpRoots::List(roots) => roots,
							McpRoots::Provider(provider) => provider().await?,
						};
						Ok(json!({ "roots": roots }))
					})
				}),
			);
		}
		Self {
			inner: Rc::new(Inner {
				options,
				state: RefCell::new(State {
					state: ClientState::Idle,
					transport: None,
					next_request_id: 1.0,
					server_info: None,
					server_capabilities: None,
					instructions: None,
					protocol_version: None,
					pending: IndexMap::new(),
					progress_requests: IndexMap::new(),
					incoming: IndexMap::new(),
					request_handlers,
					notification_listeners: IndexMap::new(),
					error_listeners: Vec::new(),
					close_listeners: Vec::new(),
					next_listener_id: 0,
					disposers: Vec::new(),
				}),
			}),
		}
	}

	/// `client.options`.
	pub fn options(&self) -> &McpClientOptions {
		&self.inner.options
	}

	pub fn connection_state(&self) -> ClientState {
		self.inner.state.borrow().state
	}

	pub fn server_info(&self) -> Option<Implementation> {
		self.inner.state.borrow().server_info.clone()
	}

	pub fn server_capabilities(&self) -> Option<ServerCapabilities> {
		self.inner.state.borrow().server_capabilities.clone()
	}

	pub fn instructions(&self) -> Option<String> {
		self.inner.state.borrow().instructions.clone()
	}

	pub fn protocol_version(&self) -> Option<String> {
		self.inner.state.borrow().protocol_version.clone()
	}

	pub fn connect(&self, transport: Rc<dyn McpTransport>) -> LocalBoxFuture<'static, PiResult<InitializeResult>> {
		let state = self.connection_state();
		if state != ClientState::Idle {
			let error = PiError::new(format!("Cannot connect MCP client in {} state", state.as_str()));
			return Box::pin(async move { Err(error) });
		}
		let weak = Rc::downgrade(&self.inner);
		let disposers = vec![
			transport.on_message(Rc::new({
				let weak = weak.clone();
				move |message| {
					if let Some(inner) = weak.upgrade() {
						handle_message(&inner, message);
					}
				}
			})),
			// Transport errors are reported only. Pending requests fail when the transport closes.
			transport.on_error(Rc::new({
				let weak = weak.clone();
				move |error| {
					if let Some(inner) = weak.upgrade() {
						emit_error(&inner, error);
					}
				}
			})),
			transport.on_close(Rc::new({
				let weak = weak.clone();
				move || {
					if let Some(inner) = weak.upgrade() {
						mark_closed(&inner, McpConnectionClosedError::new());
					}
				}
			})),
		];
		{
			let mut state = self.inner.state.borrow_mut();
			state.state = ClientState::Connecting;
			state.transport = Some(transport.clone());
			state.disposers = disposers;
		}
		let client = self.clone();
		let start = transport.start();
		Box::pin(async move {
			let result = client.initialize(transport, start).await;
			if result.is_err() {
				let _ = client.close().await;
			}
			result
		})
	}

	async fn initialize(
		&self,
		transport: Rc<dyn McpTransport>,
		start: LocalBoxFuture<'static, PiResult<()>>,
	) -> PiResult<InitializeResult> {
		start.await?;
		let options = &self.inner.options;
		let mut capabilities = match serde_json::to_value(options.capabilities.clone().unwrap_or_default()) {
			Ok(Value::Object(capabilities)) => capabilities,
			_ => Map::new(),
		};
		if options.roots.is_some() && !capabilities.contains_key("roots") {
			capabilities.insert("roots".into(), json!({}));
		}
		let mut client_info = Map::new();
		client_info.insert("name".into(), json!(options.name));
		client_info.insert("version".into(), json!(options.version));
		if let Some(title) = &options.title {
			client_info.insert("title".into(), json!(title));
		}
		let mut params = Map::new();
		params.insert(
			"protocolVersion".into(),
			json!(options.protocol_version.unwrap_or(LATEST_PROTOCOL_VERSION)),
		);
		params.insert("capabilities".into(), Value::Object(capabilities));
		params.insert("clientInfo".into(), Value::Object(client_info));
		let result = validate_initialize_result(
			request_internal(
				&self.inner,
				"initialize",
				Some(params),
				McpRequestOptions::default(),
				true,
			)
			.await?,
		)?;
		if !SUPPORTED_PROTOCOL_VERSIONS.contains(&result.protocol_version.as_str()) {
			return Err(PiError::new(format!(
				"MCP server selected unsupported protocol version {}",
				result.protocol_version
			)));
		}
		{
			let mut state = self.inner.state.borrow_mut();
			state.protocol_version = Some(result.protocol_version.clone());
			state.server_info = Some(result.server_info.clone());
			state.server_capabilities = Some(result.capabilities.clone());
			state.instructions = result.instructions.clone();
		}
		transport.set_protocol_version(&result.protocol_version);
		notify_internal(&self.inner, "notifications/initialized", None, true).await?;
		self.inner.state.borrow_mut().state = ClientState::Connected;
		Ok(result)
	}

	pub fn request(
		&self,
		method: &str,
		params: Option<Map<String, Value>>,
		options: McpRequestOptions,
	) -> LocalBoxFuture<'static, PiResult<Value>> {
		request_internal(&self.inner, method, params, options, false)
	}

	pub fn notify(&self, method: &str, params: Option<Map<String, Value>>) -> LocalBoxFuture<'static, PiResult<()>> {
		notify_internal(&self.inner, method, params, false)
	}

	pub fn set_request_handler(&self, method: &str, handler: RequestHandler) -> Unsubscribe {
		self.inner
			.state
			.borrow_mut()
			.request_handlers
			.insert(method.to_string(), handler.clone());
		let weak = Rc::downgrade(&self.inner);
		let method = method.to_string();
		Box::new(move || {
			let Some(inner) = weak.upgrade() else {
				return;
			};
			let mut state = inner.state.borrow_mut();
			if state
				.request_handlers
				.get(&method)
				.is_some_and(|current| Rc::ptr_eq(current, &handler))
			{
				state.request_handlers.shift_remove(&method);
			}
		})
	}

	pub fn on_notification(&self, method: &str, listener: NotificationListener) -> Unsubscribe {
		let id = {
			let mut state = self.inner.state.borrow_mut();
			let id = state.next_listener_id;
			state.next_listener_id += 1;
			state
				.notification_listeners
				.entry(method.to_string())
				.or_default()
				.push((id, listener));
			id
		};
		let weak = Rc::downgrade(&self.inner);
		let method = method.to_string();
		Box::new(move || {
			let Some(inner) = weak.upgrade() else {
				return;
			};
			let mut state = inner.state.borrow_mut();
			let Some(listeners) = state.notification_listeners.get_mut(&method) else {
				return;
			};
			listeners.retain(|(listener_id, _)| *listener_id != id);
			if listeners.is_empty() {
				state.notification_listeners.shift_remove(&method);
			}
		})
	}

	pub fn on_error(&self, listener: ErrorListener) -> Unsubscribe {
		let id = {
			let mut state = self.inner.state.borrow_mut();
			let id = state.next_listener_id;
			state.next_listener_id += 1;
			state.error_listeners.push((id, listener));
			id
		};
		let weak = Rc::downgrade(&self.inner);
		Box::new(move || {
			if let Some(inner) = weak.upgrade() {
				inner
					.state
					.borrow_mut()
					.error_listeners
					.retain(|(listener_id, _)| *listener_id != id);
			}
		})
	}

	/// Called once when the connection closes, whether the transport dropped or `close()` was called.
	pub fn on_close(&self, listener: CloseListener) -> Unsubscribe {
		let id = {
			let mut state = self.inner.state.borrow_mut();
			let id = state.next_listener_id;
			state.next_listener_id += 1;
			state.close_listeners.push((id, listener));
			id
		};
		let weak = Rc::downgrade(&self.inner);
		Box::new(move || {
			if let Some(inner) = weak.upgrade() {
				inner
					.state
					.borrow_mut()
					.close_listeners
					.retain(|(listener_id, _)| *listener_id != id);
			}
		})
	}

	pub fn ping(&self, options: McpRequestOptions) -> LocalBoxFuture<'static, PiResult<()>> {
		let request = self.request("ping", None, options);
		Box::pin(async move {
			request.await?;
			Ok(())
		})
	}

	pub async fn list_tools(&self, options: McpRequestOptions) -> PiResult<Vec<Tool>> {
		let items = self.list_all("tools/list", "tools", is_tool, options).await?;
		items_as("tools/list", items.into_iter().map(Value::Object).collect())
	}

	/// Every resource, following `nextCursor` through all pages.
	pub async fn list_resources(&self, options: McpRequestOptions) -> PiResult<Vec<Resource>> {
		to_resources(
			self.list_all("resources/list", "resources", is_resource, options)
				.await?,
		)
	}

	/// One page of resources, starting at `cursor`.
	pub async fn list_resources_page(
		&self,
		cursor: Option<&str>,
		options: McpRequestOptions,
	) -> PiResult<ListResourcesResult> {
		let page = self
			.list_page("resources/list", "resources", is_resource, cursor, options)
			.await?;
		Ok(ListResourcesResult {
			resources: to_resources(page.items)?,
			next_cursor: page.next_cursor,
			meta: None,
		})
	}

	/// Every resource template, following `nextCursor` through all pages.
	pub async fn list_resource_templates(&self, options: McpRequestOptions) -> PiResult<Vec<ResourceTemplate>> {
		let templates = self
			.list_all(
				"resources/templates/list",
				"resourceTemplates",
				is_resource_template,
				options,
			)
			.await?;
		to_resource_templates(templates)
	}

	/// One page of resource templates, starting at `cursor`.
	pub async fn list_resource_templates_page(
		&self,
		cursor: Option<&str>,
		options: McpRequestOptions,
	) -> PiResult<ListResourceTemplatesResult> {
		let page = self
			.list_page(
				"resources/templates/list",
				"resourceTemplates",
				is_resource_template,
				cursor,
				options,
			)
			.await?;
		Ok(ListResourceTemplatesResult {
			resource_templates: to_resource_templates(page.items)?,
			next_cursor: page.next_cursor,
			meta: None,
		})
	}

	pub fn read_resource(
		&self,
		uri: &str,
		options: McpRequestOptions,
	) -> LocalBoxFuture<'static, PiResult<ReadResourceResult>> {
		let mut params = Map::new();
		params.insert("uri".into(), json!(uri));
		let request = self.request("resources/read", Some(params), options);
		Box::pin(async move { validate_read_resource_result(request.await?) })
	}

	async fn list_page(
		&self,
		method: &str,
		key: &str,
		is_item: IsItem,
		cursor: Option<&str>,
		options: McpRequestOptions,
	) -> PiResult<ListPage> {
		let params = cursor.map(|cursor| {
			let mut params = Map::new();
			params.insert("cursor".into(), json!(cursor));
			params
		});
		validate_list_page(method, key, self.request(method, params, options).await?, is_item)
	}

	/// Every item of a paginated list method.
	async fn list_all(
		&self,
		method: &str,
		key: &str,
		is_item: IsItem,
		options: McpRequestOptions,
	) -> PiResult<Vec<Map<String, Value>>> {
		let mut items = Vec::new();
		let mut cursors: IndexSet<String> = IndexSet::new();
		let mut cursor: Option<String> = None;
		for _ in 0..MAX_LIST_PAGES {
			let page = self
				.list_page(method, key, is_item, cursor.as_deref(), options.clone())
				.await?;
			items.extend(page.items);
			let Some(next_cursor) = page.next_cursor else {
				return Ok(items);
			};
			if cursors.contains(&next_cursor) {
				return Err(PiError::new(format!(
					"MCP {method} returned duplicate cursor: {next_cursor}"
				)));
			}
			cursors.insert(next_cursor.clone());
			cursor = Some(next_cursor);
		}
		Err(PiError::new(format!("MCP {method} exceeded {MAX_LIST_PAGES} pages")))
	}

	pub fn call_tool(
		&self,
		name: &str,
		args: Option<Map<String, Value>>,
		options: McpRequestOptions,
	) -> LocalBoxFuture<'static, PiResult<CallToolResult>> {
		let mut params = Map::new();
		params.insert("name".into(), json!(name));
		if let Some(args) = args {
			params.insert("arguments".into(), Value::Object(args));
		}
		let request = self.request("tools/call", Some(params), options);
		Box::pin(async move { validate_call_tool_result(request.await?) })
	}

	pub fn close(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let (transport, disposers) = {
			let mut state = self.inner.state.borrow_mut();
			(state.transport.take(), std::mem::take(&mut state.disposers))
		};
		for dispose in disposers {
			dispose();
		}
		mark_closed(&self.inner, McpConnectionClosedError::new());
		match transport {
			Some(transport) => transport.close(),
			None => Box::pin(async { Ok(()) }),
		}
	}
}

fn request_internal(
	inner: &Rc<Inner>,
	method: &str,
	params: Option<Map<String, Value>>,
	options: McpRequestOptions,
	allow_connecting: bool,
) -> LocalBoxFuture<'static, PiResult<Value>> {
	let transport = match require_transport(inner, allow_connecting) {
		Ok(transport) => transport,
		Err(error) => return Box::pin(async move { Err(error) }),
	};
	if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
		return Box::pin(async { Err(McpAbortError::new()) });
	}
	let id = {
		let mut state = inner.state.borrow_mut();
		let id = JsonRpcId::Number(state.next_request_id);
		state.next_request_id += 1.0;
		id
	};
	let progress_token = options.on_progress.as_ref().map(|_| id.clone());
	let request_params = match &progress_token {
		None => params,
		Some(progress_token) => {
			let mut params = params.unwrap_or_default();
			let mut meta = match params.get("_meta") {
				Some(Value::Object(meta)) => meta.clone(),
				_ => Map::new(),
			};
			meta.insert(
				"progressToken".into(),
				serde_json::to_value(progress_token).expect("ids serialize"),
			);
			params.insert("_meta".into(), Value::Object(meta));
			Some(params)
		}
	};
	let message = JsonRpcMessage::Request(JsonRpcRequest {
		jsonrpc: Default::default(),
		id: id.clone(),
		method: method.to_string(),
		params: request_params.map(Value::Object),
	});
	let (sender, receiver) = oneshot::channel();
	// The spec forbids cancelling `initialize`.
	let cancellable = method != "initialize";
	let abort_listener = options.signal.as_ref().map(|signal| {
		let weak = Rc::downgrade(inner);
		let id = id.clone();
		signal.add_listener(move |reason| {
			if let Some(inner) = weak.upgrade() {
				cancel_pending(
					&inner,
					&id,
					McpAbortError::new(),
					cancellable,
					Some(js_error_string(reason)),
				);
			}
		})
	});
	let send_controller = AbortController::new();
	let send_signal = send_controller.signal();
	inner.state.borrow_mut().pending.insert(
		id.clone(),
		PendingRequest {
			sender,
			timeout_ms: options
				.timeout_ms
				.or(inner.options.request_timeout_ms)
				.unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS),
			timer: None,
			signal: options.signal.clone(),
			abort_listener,
			send_controller,
			cancellable,
			on_progress: options.on_progress.clone(),
			progress_token: progress_token.clone(),
		},
	);
	if let Some(progress_token) = progress_token {
		inner
			.state
			.borrow_mut()
			.progress_requests
			.insert(progress_token, id.clone());
	}
	arm_timeout(inner, &id);
	let send = transport.send(
		message,
		McpTransportSendOptions {
			signal: Some(send_signal),
		},
	);
	let weak = Rc::downgrade(inner);
	pi_host::spawn_local(async move {
		if let Err(error) = send.await
			&& let Some(inner) = weak.upgrade()
		{
			cancel_pending(&inner, &id, error, false, None);
		}
	});
	Box::pin(async move {
		match receiver.await {
			Ok(result) => result,
			Err(_) => Err(McpConnectionClosedError::new()),
		}
	})
}

fn notify_internal(
	inner: &Rc<Inner>,
	method: &str,
	params: Option<Map<String, Value>>,
	allow_connecting: bool,
) -> LocalBoxFuture<'static, PiResult<()>> {
	match require_transport(inner, allow_connecting) {
		Ok(transport) => transport.send(
			JsonRpcMessage::notification(method, params.map(Value::Object)),
			McpTransportSendOptions::default(),
		),
		Err(error) => Box::pin(async move { Err(error) }),
	}
}

fn require_transport(inner: &Rc<Inner>, allow_connecting: bool) -> PiResult<Rc<dyn McpTransport>> {
	let state = inner.state.borrow();
	if let Some(transport) = &state.transport
		&& (state.state == ClientState::Connected || (allow_connecting && state.state == ClientState::Connecting))
	{
		return Ok(transport.clone());
	}
	Err(McpConnectionClosedError::with_message(format!(
		"MCP client is {}",
		state.state.as_str()
	)))
}

fn handle_message(inner: &Rc<Inner>, message: &JsonRpcMessage) {
	match message {
		JsonRpcMessage::Response(response) => handle_response(inner, response),
		JsonRpcMessage::Request(request) => {
			let inner = inner.clone();
			let request = request.clone();
			pi_host::spawn_local(async move { handle_request(inner, request).await });
		}
		JsonRpcMessage::Notification(notification) => {
			handle_notification(inner, &notification.method, notification.params.as_ref())
		}
	}
}

fn handle_response(inner: &Rc<Inner>, message: &JsonRpcResponse) {
	let Some(entry) = remove_pending(inner, message.id()) else {
		emit_error(
			inner,
			&PiError::new(format!(
				"Received response for unknown MCP request {}",
				message.id().to_js_string()
			)),
		);
		return;
	};
	let result = match message {
		JsonRpcResponse::Error(response) => Err(McpError::new(
			response.error.code,
			response.error.message.clone(),
			response.error.data.clone(),
		)),
		JsonRpcResponse::Success(response) => Ok(response.result.clone()),
	};
	let _ = entry.sender.send(result);
}

async fn handle_request(inner: Rc<Inner>, message: JsonRpcRequest) {
	let (transport, handler) = {
		let state = inner.state.borrow();
		let Some(transport) = state.transport.clone() else {
			return;
		};
		(transport, state.request_handlers.get(&message.method).cloned())
	};
	let Some(handler) = handler else {
		let response = JsonRpcMessage::error(
			message.id.clone(),
			JsonRpcErrorObject {
				code: JSON_RPC_ERROR_CODES::METHOD_NOT_FOUND,
				message: format!("Method not found: {}", message.method),
				data: None,
			},
		);
		if let Err(error) = transport.send(response, McpTransportSendOptions::default()).await {
			emit_error(&inner, &error);
		}
		return;
	};
	let controller = AbortController::new();
	inner
		.state
		.borrow_mut()
		.incoming
		.insert(message.id.clone(), controller.clone());
	let outcome = async {
		let result = handler(
			message.params.clone(),
			RequestHandlerContext {
				signal: controller.signal(),
			},
		)
		.await?;
		let result = if result.is_null() { json!({}) } else { result };
		transport
			.send(
				JsonRpcMessage::result(message.id.clone(), result),
				McpTransportSendOptions::default(),
			)
			.await
	}
	.await;
	if let Err(error) = outcome {
		let response_error = if McpError::is(&error) {
			JsonRpcErrorObject {
				code: McpError::code(&error).unwrap_or(JSON_RPC_ERROR_CODES::INTERNAL_ERROR),
				message: error.message.clone(),
				data: McpError::data(&error),
			}
		} else {
			JsonRpcErrorObject {
				code: JSON_RPC_ERROR_CODES::INTERNAL_ERROR,
				message: error.message.clone(),
				data: None,
			}
		};
		if let Err(send_error) = transport
			.send(
				JsonRpcMessage::error(message.id.clone(), response_error),
				McpTransportSendOptions::default(),
			)
			.await
		{
			emit_error(&inner, &send_error);
		}
	}
	inner.state.borrow_mut().incoming.shift_remove(&message.id);
}

fn handle_notification(inner: &Rc<Inner>, method: &str, params: Option<&Value>) {
	if method == "notifications/progress" {
		handle_progress(inner, params);
	} else if method == "notifications/cancelled" {
		handle_cancelled(inner, params);
	}
	let snapshot = inner
		.state
		.borrow()
		.notification_listeners
		.get(method)
		.cloned()
		.unwrap_or_default();
	for (id, listener) in snapshot {
		let registered = inner
			.state
			.borrow()
			.notification_listeners
			.get(method)
			.is_some_and(|listeners| listeners.iter().any(|(listener_id, _)| *listener_id == id));
		if !registered {
			continue;
		}
		if let Err(error) = listener(params) {
			emit_error(inner, &error);
		}
	}
}

fn handle_progress(inner: &Rc<Inner>, params: Option<&Value>) {
	let Some(params) = params.filter(|params| {
		params.get("progressToken").is_some_and(is_json_rpc_id) && params.get("progress").is_some_and(Value::is_number)
	}) else {
		return;
	};
	let token = JsonRpcId::from_value(&params["progressToken"]).expect("checked by isJsonRpcId");
	let (request_id, on_progress) = {
		let state = inner.state.borrow();
		let Some(request_id) = state.progress_requests.get(&token).cloned() else {
			return;
		};
		let Some(entry) = state.pending.get(&request_id) else {
			return;
		};
		(request_id, entry.on_progress.clone())
	};
	arm_timeout(inner, &request_id);
	let (Some(on_progress), Ok(progress)) = (
		on_progress,
		serde_json::from_value::<ProgressNotification>(params.clone()),
	) else {
		return;
	};
	if let Err(error) = on_progress(&progress) {
		emit_error(inner, &error);
	}
}

fn handle_cancelled(inner: &Rc<Inner>, params: Option<&Value>) {
	let Some(request_id) = params
		.filter(|params| params.is_object())
		.and_then(|params| params.get("requestId"))
		.and_then(JsonRpcId::from_value)
	else {
		return;
	};
	let controller = inner.state.borrow().incoming.get(&request_id).cloned();
	let Some(controller) = controller else {
		return;
	};
	match params.and_then(|params| params.get("reason")) {
		None => controller.abort(),
		Some(Value::String(reason)) => controller.abort_with(PiError::named("", reason.clone())),
		Some(reason) => controller.abort_with(PiError::named("", js_string(Some(reason)))),
	}
}

fn arm_timeout(inner: &Rc<Inner>, id: &JsonRpcId) {
	let (timeout_ms, cancellable) = {
		let mut state = inner.state.borrow_mut();
		let Some(entry) = state.pending.get_mut(id) else {
			return;
		};
		if let Some(timer) = entry.timer.take() {
			timer.abort();
		}
		(entry.timeout_ms, entry.cancellable)
	};
	if !timeout_ms.is_finite() || timeout_ms <= 0.0 {
		return;
	}
	let (sleep, handle) = abortable(pi_host::sleep(timeout_ms, None));
	if let Some(entry) = inner.state.borrow_mut().pending.get_mut(id) {
		entry.timer = Some(handle);
	}
	let weak: Weak<Inner> = Rc::downgrade(inner);
	let id = id.clone();
	pi_host::spawn_local(async move {
		if sleep.await.is_ok()
			&& let Some(inner) = weak.upgrade()
		{
			cancel_pending(
				&inner,
				&id,
				McpTimeoutError::new(timeout_ms),
				cancellable,
				Some("Request timed out".into()),
			);
		}
	});
}

fn cancel_pending(inner: &Rc<Inner>, id: &JsonRpcId, error: PiError, notify_server: bool, reason: Option<String>) {
	let Some(entry) = remove_pending(inner, id) else {
		return;
	};
	let _ = entry.sender.send(Err(error));
	let transport = inner.state.borrow().transport.clone();
	if notify_server && let Some(transport) = transport {
		let mut params = Map::new();
		params.insert("requestId".into(), serde_json::to_value(id).expect("ids serialize"));
		if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
			params.insert("reason".into(), json!(reason));
		}
		let send = transport.send(
			JsonRpcMessage::notification("notifications/cancelled", Some(Value::Object(params))),
			McpTransportSendOptions::default(),
		);
		let weak = Rc::downgrade(inner);
		pi_host::spawn_local(async move {
			if let Err(error) = send.await
				&& let Some(inner) = weak.upgrade()
			{
				emit_error(&inner, &error);
			}
		});
	}
}

fn remove_pending(inner: &Rc<Inner>, id: &JsonRpcId) -> Option<PendingRequest> {
	let mut entry = {
		let mut state = inner.state.borrow_mut();
		let entry = state.pending.shift_remove(id)?;
		if let Some(progress_token) = &entry.progress_token {
			state.progress_requests.shift_remove(progress_token);
		}
		entry
	};
	if let Some(timer) = entry.timer.take() {
		timer.abort();
	}
	if let (Some(signal), Some(listener)) = (&entry.signal, entry.abort_listener.take()) {
		signal.remove_listener(listener);
	}
	entry.send_controller.abort();
	Some(entry)
}

/// Idempotent: rejects in-flight requests, aborts server requests we are serving, and flips the state.
fn mark_closed(inner: &Rc<Inner>, error: PiError) {
	let (was_closed, ids, incoming) = {
		let mut state = inner.state.borrow_mut();
		let was_closed = state.state == ClientState::Closed;
		state.state = ClientState::Closed;
		let ids: Vec<JsonRpcId> = state.pending.keys().cloned().collect();
		(was_closed, ids, std::mem::take(&mut state.incoming))
	};
	for id in ids {
		if let Some(entry) = remove_pending(inner, &id) {
			let _ = entry.sender.send(Err(error.clone()));
		}
	}
	for controller in incoming.values() {
		controller.abort_with(error.clone());
	}
	if was_closed {
		return;
	}
	let listeners = inner.state.borrow().close_listeners.clone();
	for (_, listener) in listeners {
		if let Err(listener_error) = listener() {
			emit_error(inner, &listener_error);
		}
	}
}

fn emit_error(inner: &Rc<Inner>, error: &PiError) {
	let snapshot = inner.state.borrow().error_listeners.clone();
	for (id, listener) in snapshot {
		let registered = inner
			.state
			.borrow()
			.error_listeners
			.iter()
			.any(|(listener_id, _)| *listener_id == id);
		if registered {
			listener(error);
		}
	}
}
