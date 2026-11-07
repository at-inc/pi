//! Exports of `oauth/callback`: `OAuthCallbackServer` over the Rust server, which listens through the host's
//! `listenHttp`.

use std::rc::Rc;

use js_sys::{Function, Promise};
use pi_mcp::oauth::callback::RenderPageFunction;
use pi_mcp::oauth::{OAuthCallbackPage, OAuthCallbackServer, OAuthCallbackServerOptions};
use serde_json::json;
use wasm_bindgen::prelude::*;

use super::{function, optional};

/// `OAuthCallbackServer.listen(options)`, resolving to an `OAuthCallbackServerHandle`.
#[wasm_bindgen(js_name = mcpListenOAuthCallbackServer)]
pub fn listen(options: JsValue) -> Promise {
	super::rejecting(|| {
		let options = OAuthCallbackServerOptions {
			host: optional(&options, "host")?,
			redirect_host: optional(&options, "redirectHost")?,
			port: optional(&options, "port")?,
			path: optional(&options, "path")?,
			extra_paths: optional(&options, "extraPaths")?,
			timeout_ms: optional(&options, "timeoutMs")?,
			render_page: function(&options, "renderPage").map(render_page),
		};
		Ok(wasm_bindgen_futures::future_to_promise(async move {
			let server = OAuthCallbackServer::listen(options)
				.await
				.map_err(|error| crate::error::to_js(&error))?;
			Ok(OAuthCallbackServerHandle { inner: Rc::new(server) }.into())
		}))
	})
}

/// `renderPage(page)` with the TS page object (`{ ok: true }` or `{ ok: false, message, details? }`).
fn render_page(render: Function) -> RenderPageFunction {
	Rc::new(move |page: &OAuthCallbackPage| {
		let page = match page {
			OAuthCallbackPage::Ok => json!({ "ok": true }),
			OAuthCallbackPage::Failed { message, details } => match details {
				Some(details) => json!({ "ok": false, "message": message, "details": details }),
				None => json!({ "ok": false, "message": message }),
			},
		};
		crate::json::to_js(&page)
			.ok()
			.and_then(|page| render.call1(&JsValue::UNDEFINED, &page).ok())
			.and_then(|html| html.as_string())
			.unwrap_or_default()
	})
}

/// A Rust `OAuthCallbackServer`, owned by the facade's class.
#[wasm_bindgen]
pub struct OAuthCallbackServerHandle {
	inner: Rc<OAuthCallbackServer>,
}

#[wasm_bindgen]
impl OAuthCallbackServerHandle {
	/// `server.redirectUrl`.
	#[wasm_bindgen(js_name = redirectUrl)]
	pub fn redirect_url(&self) -> String {
		self.inner.redirect_url().into()
	}

	/// `server.waitForCallback(state, path)`: throws when `state` is already pending, then resolves to
	/// `{ code, state, iss? }`.
	#[wasm_bindgen(js_name = waitForCallback)]
	pub fn wait_for_callback(&self, state: &str, path: Option<String>) -> Result<Promise, JsValue> {
		let pending = crate::options::throw(self.inner.wait_for_callback(state, path.as_deref()))?;
		Ok(super::promise(async move {
			let callback = pending.await?;
			let mut value = json!({ "code": callback.code, "state": callback.state });
			if let Some(iss) = callback.iss {
				value["iss"] = iss.into();
			}
			Ok(value)
		}))
	}

	/// `server.close()`.
	pub fn close(&self) -> Promise {
		let server = self.inner.clone();
		super::unit_promise(async move { server.close().await })
	}
}
