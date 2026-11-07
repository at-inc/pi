//! Exports of `auth/oauth/callback-server`: the Rust server with JS values as the completed result.

use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_ai::auth::oauth::callback_server::{
	CallbackOrManualInput, ManualInputPrompt, OAuthCallbackServer, OAuthCallbackServerOptions,
	start_oauth_callback_server, wait_for_callback_or_manual_input,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::auth::{optional_field, promise, provider_interaction_from_js};
use crate::options::arg;

/// `startOAuthCallbackServer(options)`: resolves with the server's handle once it listens.
#[wasm_bindgen(js_name = aiStartOAuthCallbackServer)]
pub fn ai_start_oauth_callback_server(options: JsValue) -> Result<Promise, JsValue> {
	let field = |name: &str| Reflect::get(&options, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let complete: Function = field("complete")
		.dyn_into()
		.map_err(|_| JsValue::from(js_sys::TypeError::new("options.complete is not a function")))?;
	let timeout = field("timeoutMs");
	let options = OAuthCallbackServerOptions {
		provider_name: arg(field("providerName"))?,
		host: arg(field("host"))?,
		port: arg(field("port"))?,
		path: arg(field("path"))?,
		redirect_host: optional_field(&options, "redirectHost")?,
		state: optional_field(&options, "state")?,
		complete: Rc::new(move |code: String| {
			let called = complete
				.call1(&JsValue::UNDEFINED, &code.into())
				.map_err(crate::error::from_js);
			Box::pin(async move {
				JsFuture::from(Promise::resolve(&called?))
					.await
					.map_err(crate::error::from_js)
			})
		}),
		signal: crate::abort::from_js(&field("signal")),
		timeout_ms: timeout.as_f64(),
	};
	let started = start_oauth_callback_server(options);
	Ok(promise(async move {
		Ok(OAuthCallbackServerHandle { inner: started.await? }.into())
	}))
}

/// A started callback server, owned by the facade's server object.
#[wasm_bindgen]
pub struct OAuthCallbackServerHandle {
	inner: OAuthCallbackServer<JsValue>,
}

#[wasm_bindgen]
impl OAuthCallbackServerHandle {
	/// Another handle to the same server, for passing by value.
	pub fn share(&self) -> OAuthCallbackServerHandle {
		OAuthCallbackServerHandle {
			inner: self.inner.clone(),
		}
	}

	/// `server.redirectUri`.
	#[wasm_bindgen(getter, js_name = redirectUri)]
	pub fn redirect_uri(&self) -> String {
		self.inner.redirect_uri.clone()
	}

	/// `server.wait()`: the completed value, or `undefined` after `cancel()`.
	pub fn wait(&self) -> Promise {
		let wait = self.inner.wait();
		promise(async move { Ok(wait.await?.unwrap_or(JsValue::UNDEFINED)) })
	}

	/// `server.cancel()`.
	pub fn cancel(&self) {
		self.inner.cancel();
	}

	/// `server.close()`.
	pub fn close(&self) {
		self.inner.close();
	}
}

/// `waitForCallbackOrManualInput(interaction, callback, prompt)`: `{ type: "callback", value }` or
/// `{ type: "manual", input }`.
#[wasm_bindgen(js_name = aiWaitForCallbackOrManualInput)]
pub fn ai_wait_for_callback_or_manual_input(
	interaction: JsValue,
	callback: Option<OAuthCallbackServerHandle>,
	prompt: JsValue,
) -> Result<Promise, JsValue> {
	let field = |name: &str| Reflect::get(&prompt, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let prompt = ManualInputPrompt {
		message: arg(field("message"))?,
		placeholder: arg(field("placeholder"))?,
	};
	let interaction = provider_interaction_from_js(&interaction);
	Ok(promise(async move {
		let result =
			wait_for_callback_or_manual_input(&interaction, callback.as_ref().map(|c| &c.inner), prompt).await?;
		let object = Object::new();
		let (kind, key, value) = match result {
			CallbackOrManualInput::Callback { value } => ("callback", "value", value),
			CallbackOrManualInput::Manual { input } => ("manual", "input", input.into()),
		};
		Reflect::set(&object, &"type".into(), &kind.into()).map_err(crate::error::from_js)?;
		Reflect::set(&object, &key.into(), &value).map_err(crate::error::from_js)?;
		Ok(object.into())
	}))
}
