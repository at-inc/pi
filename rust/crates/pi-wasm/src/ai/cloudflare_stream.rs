//! Exports of `providers/cloudflare-stream`: the Cloudflare endpoint resolution and the wrappers over a chat or
//! classifier implementation, which is a facade `*Api()` value or a JS object.

use pi_ai::providers::cloudflare_stream;
use pi_ai::{AnyModel, Model, ProviderEnv};
use pi_host::PiError;
use wasm_bindgen::prelude::*;

use super::models::{NativeClassifier, NativeStreams, provider_classifier, provider_streams};
use crate::options::{arg, throw};

/// The `baseUrl` `resolveCloudflareModel(model, env)` gives the model; the facade keeps the model when it is unchanged.
/// A model of an unknown type is read as a chat model, whose fields the resolution does not read.
#[wasm_bindgen(js_name = aiResolveCloudflareBaseUrl)]
pub fn resolve_cloudflare_base_url(model: JsValue, env: JsValue) -> Result<String, JsValue> {
	let env: Option<ProviderEnv> = arg(env)?;
	let env = env.as_ref();
	let base_url = match arg::<AnyModel>(model)? {
		AnyModel::Chat(model) => cloudflare_stream::resolve_cloudflare_model(&model, env)
			.base_url
			.clone(),
		AnyModel::Image(model) => cloudflare_stream::resolve_cloudflare_model(&model, env)
			.base_url
			.clone(),
		AnyModel::Classifier(model) => cloudflare_stream::resolve_cloudflare_model(&model, env)
			.base_url
			.clone(),
		AnyModel::Other(model) => {
			let model: Model =
				throw(serde_json::from_value(model).map_err(|error| PiError::type_error(error.to_string())))?;
			cloudflare_stream::resolve_cloudflare_model(&model, env)
				.base_url
				.clone()
		}
	};
	Ok(base_url)
}

/// `cloudflareStreams(streams)`.
#[wasm_bindgen(js_name = aiCloudflareStreams)]
pub fn cloudflare_streams(native: Option<NativeStreams>, object: JsValue) -> NativeStreams {
	NativeStreams::new(cloudflare_stream::cloudflare_streams(provider_streams(native, object)))
}

/// `cloudflareClassifier(classifier)`.
#[wasm_bindgen(js_name = aiCloudflareClassifier)]
pub fn cloudflare_classifier(native: Option<NativeClassifier>, object: JsValue) -> NativeClassifier {
	NativeClassifier::new(cloudflare_stream::cloudflare_classifier(provider_classifier(
		native, object,
	)))
}
