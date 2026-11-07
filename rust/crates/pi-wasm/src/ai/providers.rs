//! Exports of `providers/*`: the built-in provider factories, the catalog reads and collections of `providers/all`, the
//! generated `*.models` catalogs, and the helpers some provider modules export (`openRouterCost`, the Radius config).

use pi_ai::providers;
use pi_ai::providers::radius::RadiusProviderOptions;
use pi_ai::providers::radius_config::RadiusGatewayConfig;
use pi_ai::{AnyModel, ModelType};
use wasm_bindgen::prelude::*;

use super::models::{ApiKeyAuthHandle, ModelsHandle, ProviderHandle, provider_handle};
use crate::options::{arg, throw};

/// `<name>Provider()` of `providers/<module>`, by the module's name (`"amazon-bedrock"`, `"openai-codex"`, ...).
#[wasm_bindgen(js_name = aiProvider)]
pub fn provider(module: &str) -> Result<ProviderHandle, JsValue> {
	let provider = match module {
		"amazon-bedrock" => providers::amazon_bedrock::amazon_bedrock_provider(),
		"ant-ling" => providers::ant_ling::ant_ling_provider(),
		"anthropic" => providers::anthropic::anthropic_provider(),
		"azure" => providers::azure::azure_provider(),
		"baseten" => providers::baseten::baseten_provider(),
		"cerebras" => providers::cerebras::cerebras_provider(),
		"cloudflare-ai-gateway" => providers::cloudflare_ai_gateway::cloudflare_ai_gateway_provider(),
		"cloudflare-workers-ai" => providers::cloudflare_workers_ai::cloudflare_workers_ai_provider(),
		"deepseek" => providers::deepseek::deepseek_provider(),
		"fireworks" => providers::fireworks::fireworks_provider(),
		"github-copilot" => providers::github_copilot::github_copilot_provider(),
		"google" => providers::google::google_provider(),
		"google-vertex" => providers::google_vertex::google_vertex_provider(),
		"groq" => providers::groq::groq_provider(),
		"huggingface" => providers::huggingface::huggingface_provider(),
		"kimi-coding" => providers::kimi_coding::kimi_coding_provider(),
		"meta" => providers::meta::meta_provider(),
		"minimax" => providers::minimax::minimax_provider(),
		"minimax-cn" => providers::minimax_cn::minimax_cn_provider(),
		"mistral" => providers::mistral::mistral_provider(),
		"moonshotai" => providers::moonshotai::moonshotai_provider(),
		"moonshotai-cn" => providers::moonshotai_cn::moonshotai_cn_provider(),
		"nvidia" => providers::nvidia::nvidia_provider(),
		"openai" => providers::openai::openai_provider(),
		"openai-codex" => providers::openai_codex::openai_codex_provider(),
		"openai-codex-images" => providers::openai_codex_images::openai_codex_images_provider(),
		"opencode" => providers::opencode::opencode_provider(),
		"opencode-go" => providers::opencode_go::opencode_go_provider(),
		"openrouter" => providers::openrouter::openrouter_provider(),
		"qwen-token-plan" => providers::qwen_token_plan::qwen_token_plan_provider(),
		"qwen-token-plan-cn" => providers::qwen_token_plan_cn::qwen_token_plan_cn_provider(),
		"qwen-token-plan-individual" => providers::qwen_token_plan_individual::qwen_token_plan_individual_provider(),
		"together" => providers::together::together_provider(),
		"typesafe" => providers::typesafe::typesafe_provider(),
		"vercel-ai-gateway" => providers::vercel_ai_gateway::vercel_ai_gateway_provider(),
		"xai" => providers::xai::xai_provider(),
		"xiaomi" => providers::xiaomi::xiaomi_provider(),
		"xiaomi-token-plan-ams" => providers::xiaomi_token_plan_ams::xiaomi_token_plan_ams_provider(),
		"xiaomi-token-plan-cn" => providers::xiaomi_token_plan_cn::xiaomi_token_plan_cn_provider(),
		"xiaomi-token-plan-sgp" => providers::xiaomi_token_plan_sgp::xiaomi_token_plan_sgp_provider(),
		"zai" => providers::zai::zai_provider(),
		"zai-coding-cn" => providers::zai_coding_cn::zai_coding_cn_provider(),
		_ => return Err(js_sys::Error::new(&format!("unknown provider module {module}")).into()),
	};
	Ok(provider_handle(provider))
}

/// `radiusProvider(options)`.
#[wasm_bindgen(js_name = aiRadiusProvider)]
pub fn radius_provider(id: Option<String>, name: Option<String>, gateway: Option<String>) -> ProviderHandle {
	provider_handle(providers::radius::radius_provider(Some(RadiusProviderOptions {
		id,
		name,
		gateway,
	})))
}

/// `builtinProviders()`: every built-in provider, freshly constructed.
#[wasm_bindgen(js_name = aiBuiltinProviders)]
pub fn builtin_providers() -> Vec<ProviderHandle> {
	providers::all::builtin_providers()
		.into_iter()
		.map(provider_handle)
		.collect()
}

/// `builtinImagesProviders()`: the built-in providers with image models.
#[wasm_bindgen(js_name = aiBuiltinImagesProviders)]
pub fn builtin_images_providers() -> Vec<ProviderHandle> {
	providers::all::builtin_images_providers()
		.into_iter()
		.map(provider_handle)
		.collect()
}

/// `builtinModels(options)`: a `Models` collection with every built-in provider.
#[wasm_bindgen(js_name = aiBuiltinModels)]
pub fn builtin_models(options: JsValue) -> Result<ModelsHandle, JsValue> {
	Ok(ModelsHandle::new(providers::all::builtin_models(Some(
		super::models::models_options(&options)?,
	))))
}

/// `getBuiltinModel`, `getBuiltinImageModel` or `getBuiltinClassifierModel` by model type; `undefined` when unknown.
#[wasm_bindgen(js_name = aiGetBuiltinModelOfType)]
pub fn get_builtin_model_of_type(model_type: JsValue, provider: &str, model_id: &str) -> Result<JsValue, JsValue> {
	let model = match arg::<ModelType>(model_type)? {
		ModelType::Chat => providers::all::get_builtin_model(provider, model_id).map(AnyModel::Chat),
		ModelType::Image => providers::all::get_builtin_image_model(provider, model_id).map(AnyModel::Image),
		ModelType::Classifier => {
			providers::all::get_builtin_classifier_model(provider, model_id).map(AnyModel::Classifier)
		}
	};
	model.map_or(Ok(JsValue::UNDEFINED), |model| throw(crate::json::to_js(&model)))
}

/// `getBuiltinModels`, `getBuiltinImageModels` or `getBuiltinClassifierModels` by model type.
#[wasm_bindgen(js_name = aiGetBuiltinModelsOfType)]
pub fn get_builtin_models_of_type(model_type: JsValue, provider: &str) -> Result<JsValue, JsValue> {
	match arg::<ModelType>(model_type)? {
		ModelType::Chat => throw(crate::json::to_js(&providers::all::get_builtin_models(provider))),
		ModelType::Image => throw(crate::json::to_js(&providers::all::get_builtin_image_models(provider))),
		ModelType::Classifier => throw(crate::json::to_js(&providers::all::get_builtin_classifier_models(
			provider,
		))),
	}
}

/// `getAllBuiltinModels(provider)`.
#[wasm_bindgen(js_name = aiGetAllBuiltinModels)]
pub fn get_all_builtin_models(provider: &str) -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&providers::all::get_all_builtin_models(provider)))
}

/// `getBuiltinProviders()`.
#[wasm_bindgen(js_name = aiGetBuiltinProviders)]
pub fn get_builtin_providers() -> Vec<String> {
	providers::all::get_builtin_providers()
		.into_iter()
		.map(String::from)
		.collect()
}

/// `getBuiltinModelDataGeneratedAt()`.
#[wasm_bindgen(js_name = aiGetBuiltinModelDataGeneratedAt)]
pub fn get_builtin_model_data_generated_at() -> Option<f64> {
	providers::all::get_builtin_model_data_generated_at()
}

/// A generated `<PROVIDER>_MODELS` (`_IMAGE_MODELS`, `_CLASSIFIER_MODELS`) catalog: the provider's models of one type
/// keyed by id, empty for a provider without such models.
#[wasm_bindgen(js_name = aiBuiltinCatalog)]
pub fn builtin_catalog(model_type: JsValue, provider: &str) -> Result<JsValue, JsValue> {
	use pi_ai::models_generated::{CLASSIFIER_MODELS, IMAGE_MODELS, MODELS};
	let catalog = match arg::<ModelType>(model_type)? {
		ModelType::Chat => MODELS.get(provider).map(|catalog| crate::json::to_js(*catalog)),
		ModelType::Image => IMAGE_MODELS.get(provider).map(|catalog| crate::json::to_js(*catalog)),
		ModelType::Classifier => CLASSIFIER_MODELS
			.get(provider)
			.map(|catalog| crate::json::to_js(*catalog)),
	};
	catalog.map_or_else(|| Ok(js_sys::Object::new().into()), throw)
}

/// `openRouterCost(pricing)` (`providers/openrouter-api`).
#[wasm_bindgen(js_name = aiOpenRouterCost)]
pub fn open_router_cost(pricing: JsValue) -> Result<JsValue, JsValue> {
	let pricing: Option<providers::openrouter_api::OpenRouterPricing> = if pricing.is_undefined() {
		None
	} else {
		Some(arg(pricing)?)
	};
	throw(crate::json::to_js(&providers::openrouter_api::open_router_cost(
		pricing.as_ref(),
	)))
}

/// `cloudflareWorkersAIAuth()` (`providers/cloudflare-auth`).
#[wasm_bindgen(js_name = aiCloudflareWorkersAIAuth)]
pub fn cloudflare_workers_ai_auth() -> ApiKeyAuthHandle {
	ApiKeyAuthHandle::new(providers::cloudflare_auth::cloudflare_workers_ai_auth())
}

/// `cloudflareAIGatewayAuth()` (`providers/cloudflare-auth`).
#[wasm_bindgen(js_name = aiCloudflareAIGatewayAuth)]
pub fn cloudflare_ai_gateway_auth() -> ApiKeyAuthHandle {
	ApiKeyAuthHandle::new(providers::cloudflare_auth::cloudflare_ai_gateway_auth())
}

/// `normalizeRadiusGatewayUrl(value)` (`providers/radius-config`).
#[wasm_bindgen(js_name = aiNormalizeRadiusGatewayUrl)]
pub fn normalize_radius_gateway_url(value: &str) -> String {
	providers::radius_config::normalize_radius_gateway_url(value)
}

/// `getRadiusCredentialConfig(credential)` (`providers/radius-config`).
#[wasm_bindgen(js_name = aiGetRadiusCredentialConfig)]
pub fn get_radius_credential_config(credential: JsValue) -> Result<JsValue, JsValue> {
	let credential = optional_credential(credential)?;
	let config = providers::radius_config::get_radius_credential_config(credential.as_ref());
	config.map_or(Ok(JsValue::UNDEFINED), |config| throw(crate::json::to_js(&config)))
}

/// `getRadiusModelsFromConfig(providerId, config)` (`providers/radius-config`).
#[wasm_bindgen(js_name = aiGetRadiusModelsFromConfig)]
pub fn get_radius_models_from_config(provider_id: &str, config: JsValue) -> Result<JsValue, JsValue> {
	let config: RadiusGatewayConfig = arg(config)?;
	throw(crate::json::to_js(
		&providers::radius_config::get_radius_models_from_config(provider_id, &config),
	))
}

/// `getRadiusModels(providerId, credential)` (`providers/radius-config`).
#[wasm_bindgen(js_name = aiGetRadiusModels)]
pub fn get_radius_models(provider_id: &str, credential: JsValue) -> Result<JsValue, JsValue> {
	let credential = optional_credential(credential)?;
	throw(crate::json::to_js(&providers::radius_config::get_radius_models(
		provider_id,
		credential.as_ref(),
	)))
}

fn optional_credential(credential: JsValue) -> Result<Option<pi_ai::auth::types::OAuthCredential>, JsValue> {
	if credential.is_undefined() || credential.is_null() {
		return Ok(None);
	}
	arg(credential).map(Some)
}

/// `DEFAULT_RADIUS_GATEWAY` (`providers/radius-config`).
#[wasm_bindgen(js_name = aiDefaultRadiusGateway)]
pub fn default_radius_gateway() -> String {
	providers::radius_config::DEFAULT_RADIUS_GATEWAY.to_string()
}
