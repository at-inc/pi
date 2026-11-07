//! Exports of `auth/oauth/load`: the registered bundled loaders live in the Rust core, so the Rust providers' lazy
//! OAuth sees them too.

use std::rc::Rc;

use js_sys::{Function, Promise, Reflect};
use pi_ai::auth::oauth::load::{self, OAuthFlowLoader, OAuthFlowLoaders};
use pi_ai::auth::oauth::radius::RadiusOAuthOptions;
use pi_ai::auth::types::OAuthAuth;
use pi_host::PiResult;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::auth::{OAuthAuthHandle, loaded_oauth_auth, promise};
use crate::options::arg;

/// `registerBundledOAuthFlowLoaders(loaders)`: each loader is a JS function returning an `OAuthAuth` or a Promise of one.
#[wasm_bindgen(js_name = aiRegisterBundledOAuthFlowLoaders)]
pub fn register_bundled_oauth_flow_loaders(loaders: JsValue) -> Result<(), JsValue> {
	let function = |name: &str| -> Result<Function, JsValue> {
		Reflect::get(&loaders, &name.into())?
			.dyn_into()
			.map_err(|_| JsValue::from(js_sys::TypeError::new(&format!("loaders.{name} is not a function"))))
	};
	let loader = |name: &str| -> Result<OAuthFlowLoader, JsValue> {
		let function = function(name)?;
		Ok(Rc::new(move || call_loader(&function, &[])))
	};
	let radius = function("radius")?;
	load::register_bundled_oauth_flow_loaders(OAuthFlowLoaders {
		anthropic: loader("anthropic")?,
		openai_codex: loader("openaiCodex")?,
		openai_chat_gpt: loader("openaiChatGPT")?,
		github_copilot: loader("githubCopilot")?,
		openrouter: loader("openrouter")?,
		kimi_coding: loader("kimiCoding")?,
		meta: loader("meta")?,
		xai: loader("xai")?,
		radius: Rc::new(move |options: RadiusOAuthOptions| {
			let object = js_sys::Object::new();
			let _ = Reflect::set(&object, &"name".into(), &options.name.as_str().into());
			let _ = Reflect::set(&object, &"gateway".into(), &options.gateway.as_str().into());
			call_loader(&radius, &[object.into()])
		}),
	});
	Ok(())
}

/// A JS loader's `OAuthAuth`. A loader that throws gives an auth whose methods reject with the error, as the TS
/// `load*OAuth` promise would.
fn call_loader(function: &Function, args: &[JsValue]) -> OAuthAuth {
	let args: js_sys::Array = args.iter().collect();
	match function.apply(&JsValue::UNDEFINED, &args) {
		Ok(value) => loaded_oauth_auth(value),
		Err(error) => loaded_oauth_auth(Promise::reject(&error).into()),
	}
}

/// A Promise of the handle of a loaded flow.
fn loaded(load: impl std::future::Future<Output = PiResult<OAuthAuth>> + 'static) -> Promise {
	promise(async move { Ok(OAuthAuthHandle::new(load.await?).into()) })
}

/// `loadAnthropicOAuth()`.
#[wasm_bindgen(js_name = aiLoadAnthropicOAuth)]
pub fn load_anthropic_oauth() -> Promise {
	loaded(load::load_anthropic_oauth())
}

/// `loadOpenAICodexOAuth()`.
#[wasm_bindgen(js_name = aiLoadOpenAICodexOAuth)]
pub fn load_openai_codex_oauth() -> Promise {
	loaded(load::load_openai_codex_oauth())
}

/// `loadOpenAIChatGPTOAuth()`.
#[wasm_bindgen(js_name = aiLoadOpenAIChatGPTOAuth)]
pub fn load_openai_chatgpt_oauth() -> Promise {
	loaded(load::load_openai_chatgpt_oauth())
}

/// `loadGitHubCopilotOAuth()`.
#[wasm_bindgen(js_name = aiLoadGitHubCopilotOAuth)]
pub fn load_github_copilot_oauth() -> Promise {
	loaded(load::load_github_copilot_oauth())
}

/// `loadOpenRouterOAuth()`.
#[wasm_bindgen(js_name = aiLoadOpenRouterOAuth)]
pub fn load_openrouter_oauth() -> Promise {
	loaded(load::load_openrouter_oauth())
}

/// `loadKimiCodingOAuth()`.
#[wasm_bindgen(js_name = aiLoadKimiCodingOAuth)]
pub fn load_kimi_coding_oauth() -> Promise {
	loaded(load::load_kimi_coding_oauth())
}

/// `loadMetaOAuth()`.
#[wasm_bindgen(js_name = aiLoadMetaOAuth)]
pub fn load_meta_oauth() -> Promise {
	loaded(load::load_meta_oauth())
}

/// `loadXaiOAuth()`.
#[wasm_bindgen(js_name = aiLoadXaiOAuth)]
pub fn load_xai_oauth() -> Promise {
	loaded(load::load_xai_oauth())
}

/// `loadRadiusOAuth({ name, gateway })`.
#[wasm_bindgen(js_name = aiLoadRadiusOAuth)]
pub fn load_radius_oauth(options: JsValue) -> Result<Promise, JsValue> {
	let field = |name: &str| Reflect::get(&options, &name.into()).unwrap_or(JsValue::UNDEFINED);
	Ok(loaded(load::load_radius_oauth(
		arg(field("name"))?,
		arg(field("gateway"))?,
	)))
}
