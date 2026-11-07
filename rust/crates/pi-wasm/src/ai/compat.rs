//! Exports of `compat`: the api-dispatch `stream`/`streamSimple` with env API keys and the builtin catalog reads.

use std::rc::Rc;

use pi_ai::ProviderStreams;

use wasm_bindgen::prelude::*;

use crate::events::EventSink;
use crate::options::{arg, provider_stream_options, simple_stream_options, throw};

/// `stream(model, context, options)`, sending the events to `sink`.
#[wasm_bindgen(js_name = aiStream)]
pub fn stream(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let context: pi_ai::Context = arg(context)?;
	let options = provider_stream_options(options, &model);
	crate::events::send(throw(pi_ai::compat::stream(&parsed, &context, options))?, sink);
	Ok(())
}

/// `streamSimple(model, context, options)`, sending the events to `sink`.
#[wasm_bindgen(js_name = aiStreamSimple)]
pub fn stream_simple(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let parsed = super::model(&model)?;
	let context: pi_ai::Context = arg(context)?;
	let options = simple_stream_options(options, &model);
	crate::events::send(throw(pi_ai::compat::stream_simple(&parsed, &context, options))?, sink);
	Ok(())
}

/// `getModel(provider, modelId)` (`getBuiltinModel`): a builtin catalog model, `undefined` when unknown.
#[wasm_bindgen(js_name = aiGetBuiltinModel)]
pub fn get_builtin_model(provider: &str, model_id: &str) -> Result<JsValue, JsValue> {
	match pi_ai::providers::all::get_builtin_model(provider, model_id) {
		Some(model) => throw(crate::json::to_js(&model)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `registerApiProvider(provider, sourceId)`: a JS `{ api, stream, streamSimple }`, called as JS.
#[wasm_bindgen(js_name = aiRegisterApiProvider)]
pub fn register_api_provider(provider: JsValue, source_id: Option<String>) -> Result<(), JsValue> {
	let api: String = arg(js_sys::Reflect::get(&provider, &"api".into()).unwrap_or(JsValue::UNDEFINED))?;
	let streams = Rc::new(super::models::JsStreams::new(provider));
	let simple = streams.clone();
	pi_ai::compat::register_api_provider(
		pi_ai::compat::ApiProvider {
			api,
			stream: Rc::new(move |model, context, options| streams.stream(model, context, options)),
			stream_simple: Rc::new(move |model, context, options| simple.stream_simple(model, context, options)),
		},
		source_id,
	);
	Ok(())
}

/// A registered api provider (`getApiProvider(api)`): its streams check the model's API first.
#[wasm_bindgen]
pub struct ApiProviderHandle {
	inner: Rc<pi_ai::compat::ApiProviderInternal>,
}

#[wasm_bindgen]
impl ApiProviderHandle {
	/// `provider.api`.
	#[wasm_bindgen(getter)]
	pub fn api(&self) -> String {
		self.inner.api.clone()
	}

	/// `provider.stream(model, context, options)`.
	pub fn stream(&self, model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = provider_stream_options(options, &model);
		crate::events::send(throw((self.inner.stream)(&parsed, &context, options))?, sink);
		Ok(())
	}

	/// `provider.streamSimple(model, context, options)`.
	#[wasm_bindgen(js_name = streamSimple)]
	pub fn stream_simple(
		&self,
		model: JsValue,
		context: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = simple_stream_options(options, &model);
		crate::events::send(throw((self.inner.stream_simple)(&parsed, &context, options))?, sink);
		Ok(())
	}
}

/// `getApiProvider(api)`.
#[wasm_bindgen(js_name = aiGetApiProvider)]
pub fn get_api_provider(api: &str) -> Option<ApiProviderHandle> {
	pi_ai::compat::get_api_provider(api).map(|inner| ApiProviderHandle { inner })
}

/// `getApiProviders()`.
#[wasm_bindgen(js_name = aiGetApiProviders)]
pub fn get_api_providers() -> Vec<ApiProviderHandle> {
	pi_ai::compat::get_api_providers()
		.into_iter()
		.map(|inner| ApiProviderHandle { inner })
		.collect()
}

/// `unregisterApiProviders(sourceId)`.
#[wasm_bindgen(js_name = aiUnregisterApiProviders)]
pub fn unregister_api_providers(source_id: &str) {
	pi_ai::compat::unregister_api_providers(source_id);
}

/// `registerBuiltInApiProviders()`.
#[wasm_bindgen(js_name = aiRegisterBuiltInApiProviders)]
pub fn register_built_in_api_providers() {
	pi_ai::compat::register_built_in_api_providers();
}

/// `resetApiProviders()`.
#[wasm_bindgen(js_name = aiResetApiProviders)]
pub fn reset_api_providers() {
	pi_ai::compat::reset_api_providers();
}

/// `findEnvKeys(provider, env)` (`env-api-keys`).
#[wasm_bindgen(js_name = aiFindEnvKeys)]
pub fn find_env_keys(provider: &str, env: JsValue) -> Result<Option<Vec<String>>, JsValue> {
	let env: Option<pi_ai::ProviderEnv> = if env.is_undefined() { None } else { Some(arg(env)?) };
	Ok(pi_ai::env_api_keys::find_env_keys(provider, env.as_ref()))
}

/// `getEnvApiKey(provider, env)` (`env-api-keys`).
#[wasm_bindgen(js_name = aiGetEnvApiKey)]
pub fn get_env_api_key(provider: &str, env: JsValue) -> Result<Option<String>, JsValue> {
	let env: Option<pi_ai::ProviderEnv> = if env.is_undefined() { None } else { Some(arg(env)?) };
	Ok(pi_ai::env_api_keys::get_env_api_key(provider, env.as_ref()))
}

