//! Exports of `api/azure-openai-config`.

use js_sys::Reflect;
use pi_ai::Model;
use pi_ai::api::azure_openai_config::{self, AzureEndpointOptions};
use wasm_bindgen::prelude::*;

use crate::options::{JsOptions, throw};

/// `resolveDeploymentName(model, options)`.
#[wasm_bindgen(js_name = aiResolveDeploymentName)]
pub fn resolve_deployment_name(model: JsValue, options: JsValue) -> String {
	azure_openai_config::resolve_deployment_name(&picked_model(&model), endpoint_options(options, &model).as_ref())
}

/// `resolveAzureBaseUrl(model, options)`.
#[wasm_bindgen(js_name = aiResolveAzureBaseUrl)]
pub fn resolve_azure_base_url(model: JsValue, options: JsValue) -> Result<String, JsValue> {
	throw(azure_openai_config::resolve_azure_base_url(
		&picked_model(&model),
		endpoint_options(options, &model).as_ref(),
	))
}

/// `resolveAzureConfig(model, options)`: `{ baseUrl, apiVersion }`.
#[wasm_bindgen(js_name = aiResolveAzureConfig)]
pub fn resolve_azure_config(model: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
	let config = throw(azure_openai_config::resolve_azure_config(
		&picked_model(&model),
		endpoint_options(options, &model).as_ref(),
	))?;
	throw(crate::json::to_js(&serde_json::json!({
		"baseUrl": config.base_url,
		"apiVersion": config.api_version,
	})))
}

/// The `Model` fields these functions read: the TS takes `Pick<Model, "id">` and `Pick<Model, "baseUrl">`.
fn picked_model(model: &JsValue) -> Model {
	let field = |name: &str| {
		Reflect::get(model, &name.into())
			.ok()
			.and_then(|value| value.as_string())
			.unwrap_or_default()
	};
	let mut picked = Model::default();
	picked.base.id = field("id");
	picked.base.base_url = field("baseUrl");
	picked
}

/// `AzureEndpointOptions` of a JS options object; `None` when the caller passed none.
fn endpoint_options(options: JsValue, model: &JsValue) -> Option<AzureEndpointOptions> {
	let mut options = JsOptions::new(options);
	options.is_given().then(|| AzureEndpointOptions {
		base: options.stream_options(model),
		azure_api_version: options.take("azureApiVersion"),
		azure_resource_name: options.take("azureResourceName"),
		azure_base_url: options.take("azureBaseUrl"),
		azure_deployment_name: options.take("azureDeploymentName"),
	})
}
