//! Exports of `client`: `McpClient` over a Rust client, with transports from `transport` and JS callbacks for
//! roots, request handlers, notification, error, close and progress listeners.

use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_host::{PiError, PiResult};
use pi_mcp::client::{McpRoots, ProgressListener, RequestHandler, RequestHandlerContext};
use pi_mcp::{McpClient, McpClientOptions, McpRequestOptions, SUPPORTED_PROTOCOL_VERSIONS};
use serde_json::{Map, Value};
use wasm_bindgen::prelude::*;

use super::transport::McpTransportHandle;
use super::{field, function, optional};

/// `new McpClient(options)`. `protocolVersion` must be one of `SUPPORTED_PROTOCOL_VERSIONS`, as its TS type says.
#[wasm_bindgen(js_name = mcpCreateClient)]
pub fn create_client(options: JsValue) -> Result<McpClientHandle, JsValue> {
	let protocol_version = match optional::<String>(&options, "protocolVersion")? {
		None => None,
		Some(version) => Some(
			SUPPORTED_PROTOCOL_VERSIONS
				.into_iter()
				.find(|supported| *supported == version)
				.ok_or_else(|| {
					crate::error::to_js(&PiError::type_error(format!(
						"Unsupported MCP protocol version {version}"
					)))
				})?,
		),
	};
	Ok(McpClientHandle {
		inner: McpClient::new(McpClientOptions {
			name: crate::options::arg(field(&options, "name"))?,
			version: crate::options::arg(field(&options, "version"))?,
			title: optional(&options, "title")?,
			capabilities: optional(&options, "capabilities")?,
			protocol_version,
			request_timeout_ms: optional(&options, "requestTimeoutMs")?,
			roots: roots(&options)?,
		}),
	})
}

/// `options.roots`: a list, or a function returning one (or a Promise of one).
fn roots(options: &JsValue) -> Result<Option<McpRoots>, JsValue> {
	let Some(provider) = function(options, "roots") else {
		return Ok(optional(options, "roots")?.map(McpRoots::List));
	};
	Ok(Some(McpRoots::Provider(Rc::new(move || {
		let provider = provider.clone();
		Box::pin(async move { crate::json::from_js(crate::callback::call(&provider, &JsValue::UNDEFINED, &[]).await?) })
	}))))
}

/// A Rust `McpClient`, owned by the facade's `McpClient`.
#[wasm_bindgen]
pub struct McpClientHandle {
	inner: McpClient,
}

#[wasm_bindgen]
impl McpClientHandle {
	/// `client.connectionState`.
	#[wasm_bindgen(js_name = connectionState)]
	pub fn connection_state(&self) -> String {
		self.inner.connection_state().as_str().into()
	}

	/// `client.serverInfo`.
	#[wasm_bindgen(js_name = serverInfo)]
	pub fn server_info(&self) -> Result<JsValue, JsValue> {
		super::defined(self.inner.server_info())
	}

	/// `client.serverCapabilities`.
	#[wasm_bindgen(js_name = serverCapabilities)]
	pub fn server_capabilities(&self) -> Result<JsValue, JsValue> {
		super::defined(self.inner.server_capabilities())
	}

	/// `client.instructions`.
	pub fn instructions(&self) -> Option<String> {
		self.inner.instructions()
	}

	/// `client.protocolVersion`.
	#[wasm_bindgen(js_name = protocolVersion)]
	pub fn protocol_version(&self) -> Option<String> {
		self.inner.protocol_version()
	}

	/// `client.connect(transport)`: `native` is the Rust transport of a facade transport, `js` any other one.
	pub fn connect(&self, native: Option<McpTransportHandle>, js: JsValue) -> Promise {
		super::promise(self.inner.connect(super::transport::from_js(native, js)))
	}

	/// `client.request(method, params, options)`.
	pub fn request(&self, method: &str, params: JsValue, options: JsValue) -> Promise {
		super::rejecting(|| {
			let params = params_arg(params)?;
			Ok(super::promise(self.inner.request(
				method,
				params,
				request_options(&options)?,
			)))
		})
	}

	/// `client.notify(method, params)`.
	pub fn notify(&self, method: &str, params: JsValue) -> Promise {
		super::rejecting(|| Ok(super::unit_promise(self.inner.notify(method, params_arg(params)?))))
	}

	/// `client.setRequestHandler(method, handler)`: the handler is called with `(params, { signal })`.
	#[wasm_bindgen(js_name = setRequestHandler)]
	pub fn set_request_handler(&self, method: &str, handler: Function) -> JsValue {
		let handler: RequestHandler = Rc::new(move |params: Option<Value>, context: RequestHandlerContext| {
			let handler = handler.clone();
			let signal = crate::abort::to_js(&context.signal);
			Box::pin(async move {
				let params = match params {
					Some(params) => crate::json::to_js(&params)?,
					None => JsValue::UNDEFINED,
				};
				let context = Object::new();
				let _ = Reflect::set(&context, &"signal".into(), &signal.js);
				let result = crate::callback::call(&handler, &JsValue::UNDEFINED, &[params, context.into()]).await?;
				crate::json::from_js(result)
			})
		});
		super::unsubscribe_to_js(self.inner.set_request_handler(method, handler))
	}

	/// `client.onNotification(method, listener)`.
	#[wasm_bindgen(js_name = onNotification)]
	pub fn on_notification(&self, method: &str, listener: Function) -> JsValue {
		super::unsubscribe_to_js(self.inner.on_notification(
			method,
			Rc::new(move |params: Option<&Value>| {
				let params = match params {
					Some(params) => crate::json::to_js(params)?,
					None => JsValue::UNDEFINED,
				};
				listener
					.call1(&JsValue::UNDEFINED, &params)
					.map_err(crate::error::from_js)?;
				Ok(())
			}),
		))
	}

	/// `client.onError(listener)`.
	#[wasm_bindgen(js_name = onError)]
	pub fn on_error(&self, listener: Function) -> JsValue {
		super::unsubscribe_to_js(self.inner.on_error(Rc::new(move |error: &PiError| {
			let _ = listener.call1(&JsValue::UNDEFINED, &crate::error::to_js(error));
		})))
	}

	/// `client.onClose(listener)`.
	#[wasm_bindgen(js_name = onClose)]
	pub fn on_close(&self, listener: Function) -> JsValue {
		super::unsubscribe_to_js(self.inner.on_close(Rc::new(move || {
			listener.call0(&JsValue::UNDEFINED).map_err(crate::error::from_js)?;
			Ok(())
		})))
	}

	/// `client.ping(options)`.
	pub fn ping(&self, options: JsValue) -> Promise {
		super::rejecting(|| Ok(super::unit_promise(self.inner.ping(request_options(&options)?))))
	}

	/// `client.listTools(options)`.
	#[wasm_bindgen(js_name = listTools)]
	pub fn list_tools(&self, options: JsValue) -> Promise {
		let client = self.inner.clone();
		super::rejecting(|| {
			let options = request_options(&options)?;
			Ok(super::promise(async move { client.list_tools(options).await }))
		})
	}

	/// `client.listResources(options)`.
	#[wasm_bindgen(js_name = listResources)]
	pub fn list_resources(&self, options: JsValue) -> Promise {
		let client = self.inner.clone();
		super::rejecting(|| {
			let options = request_options(&options)?;
			Ok(super::promise(async move { client.list_resources(options).await }))
		})
	}

	/// `client.listResourcesPage(cursor, options)`.
	#[wasm_bindgen(js_name = listResourcesPage)]
	pub fn list_resources_page(&self, cursor: Option<String>, options: JsValue) -> Promise {
		let client = self.inner.clone();
		super::rejecting(|| {
			let options = request_options(&options)?;
			Ok(super::promise(async move {
				client.list_resources_page(cursor.as_deref(), options).await
			}))
		})
	}

	/// `client.listResourceTemplates(options)`.
	#[wasm_bindgen(js_name = listResourceTemplates)]
	pub fn list_resource_templates(&self, options: JsValue) -> Promise {
		let client = self.inner.clone();
		super::rejecting(|| {
			let options = request_options(&options)?;
			Ok(super::promise(
				async move { client.list_resource_templates(options).await },
			))
		})
	}

	/// `client.listResourceTemplatesPage(cursor, options)`.
	#[wasm_bindgen(js_name = listResourceTemplatesPage)]
	pub fn list_resource_templates_page(&self, cursor: Option<String>, options: JsValue) -> Promise {
		let client = self.inner.clone();
		super::rejecting(|| {
			let options = request_options(&options)?;
			Ok(super::promise(async move {
				client.list_resource_templates_page(cursor.as_deref(), options).await
			}))
		})
	}

	/// `client.readResource(uri, options)`.
	#[wasm_bindgen(js_name = readResource)]
	pub fn read_resource(&self, uri: &str, options: JsValue) -> Promise {
		super::rejecting(|| {
			Ok(super::promise(
				self.inner.read_resource(uri, request_options(&options)?),
			))
		})
	}

	/// `client.callTool(name, args, options)`.
	#[wasm_bindgen(js_name = callTool)]
	pub fn call_tool(&self, name: &str, args: JsValue, options: JsValue) -> Promise {
		super::rejecting(|| {
			let args = params_arg(args)?;
			Ok(super::promise(self.inner.call_tool(
				name,
				args,
				request_options(&options)?,
			)))
		})
	}

	/// `client.close()`.
	pub fn close(&self) -> Promise {
		super::unit_promise(self.inner.close())
	}
}

/// Request `params` or tool arguments: an object, or `undefined` for none.
fn params_arg(params: JsValue) -> Result<Option<Map<String, Value>>, JsValue> {
	if params.is_undefined() || params.is_null() {
		return Ok(None);
	}
	crate::options::arg(params).map(Some)
}

/// `McpRequestOptions`: `signal`, `timeoutMs` and `onProgress`.
fn request_options(options: &JsValue) -> Result<McpRequestOptions, JsValue> {
	let on_progress = function(options, "onProgress").map(|listener| {
		Rc::new(move |progress: &pi_mcp::ProgressNotification| -> PiResult<()> {
			listener
				.call1(&JsValue::UNDEFINED, &crate::json::to_js(progress)?)
				.map_err(crate::error::from_js)?;
			Ok(())
		}) as ProgressListener
	});
	Ok(McpRequestOptions {
		signal: super::signal(options),
		timeout_ms: optional(options, "timeoutMs")?,
		on_progress,
	})
}
