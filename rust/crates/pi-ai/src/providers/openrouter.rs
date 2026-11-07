//! Port of `packages/ai/src/providers/openrouter.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.
//! - `personalizeOpenRouterCatalog` returns a copy of the catalog where the TS returns the same array.
//! - The TS casts each `/models/user` entry with a string `id` to `OpenRouterModel`; Rust reads its fields from
//!   JSON when a catalog chat model matches the entry, where the TS reads them. Limits are read as numbers, so a
//!   truthy non-number limit falls through to the next candidate instead of becoming the model's limit. Prices are
//!   read as `parseFloat(String(value))`, like the TS. An override whose `min_prompt_tokens` is not a number yields
//!   no tier, and a present `utc_*` key of any value skips its override like `!== undefined` does. Non-array
//!   `overrides` and `null` override entries reject with V8's TypeError messages.
//! - `AbortSignal.timeout(15_000)` is a host sleep raced against the request and its body read: when it wins it
//!   aborts the request's signal with a `TimeoutError`, and it is dropped when the request finishes first.

use std::future::Future;
use std::rc::Rc;

use futures::future::{Either, select};
use indexmap::IndexMap;
use pi_host::{AbortController, AbortSignal, HttpRequest, PiError, PiResult};
use serde_json::Value;

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::api::openai_completions_lazy::openai_completions_api;
use crate::api::openrouter_images_lazy::openrouter_images_api;
use crate::api::typesafe_system_one_lazy::typesafe_system_one_api;
use crate::auth::helpers::{LazyOAuthInput, env_api_key_auth, lazy_oauth};
use crate::auth::oauth::load::load_openrouter_oauth;
use crate::auth::types::{Credential, ProviderAuth};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, RefreshModelsContext, create_provider};
use crate::providers::openrouter_api::{
	OpenRouterModel, OpenRouterPricing, OpenRouterPricingOverride, OpenRouterTopProvider, open_router_cost,
};
use crate::providers::openrouter_models::{OPENROUTER_CLASSIFIER_MODELS, OPENROUTER_IMAGE_MODELS, OPENROUTER_MODELS};
use crate::types::{AnyModel, ModelType};
use crate::utils::model_operations::is_model_type;

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_USER_MODELS_TIMEOUT_MS: f64 = 15_000.0;

/// Restrict an OpenRouter catalog to the chat models the key may use, with the key's limits and
/// prices from `GET /models/user`. Image and classifier models are returned unchanged. A missing
/// limit keeps the catalog value. The request uses the provider's configured base URL, so regional
/// endpoints such as `https://us.openrouter.ai/api/v1` apply their region's filtering. When the
/// endpoint rejects the key (401/403) or does not exist (404), for example with a proxy configured
/// in models.json, the catalog is returned unchanged.
pub async fn personalize_open_router_catalog(
	catalog: &[AnyModel],
	context: &RefreshModelsContext,
) -> PiResult<Vec<AnyModel>> {
	let key = match &context.credential {
		Some(Credential::OAuth(credential)) => Some(credential.access.clone()),
		Some(Credential::ApiKey(credential)) => credential.key.clone(),
		None => None,
	};
	let Some(key) = key.filter(|key| !key.is_empty()) else {
		return Ok(catalog.to_vec());
	};
	let base_url = context.base_url.as_deref().unwrap_or(OPENROUTER_BASE_URL);
	let mut request = HttpRequest::new("GET", format!("{}/models/user", base_url.trim_end_matches('/')));
	request.set_header("accept", "application/json");
	request.set_header("authorization", format!("Bearer {key}"));
	let timeout = AbortController::new();
	request.signal = Some(AbortSignal::any(&[context.signal.clone(), timeout.signal()]));
	let body = with_timeout(
		async {
			let response = pi_host::fetch(request, None).await?;
			if matches!(response.status, 401 | 403 | 404) {
				return Ok(None);
			}
			if !response.ok() {
				return Err(PiError::new(format!(
					"OpenRouter model list request failed: {}",
					response.status
				)));
			}
			response.json().await.map(Some)
		},
		&timeout,
		OPENROUTER_USER_MODELS_TIMEOUT_MS,
	)
	.await?;
	let Some(body) = body else {
		return Ok(catalog.to_vec());
	};
	let Some(data) = body
		.as_object()
		.and_then(|body| body.get("data"))
		.and_then(Value::as_array)
	else {
		return Err(PiError::new("Invalid OpenRouter model list response"));
	};
	let mut allowed: IndexMap<&str, &Value> = IndexMap::new();
	for entry in data {
		if let Some(id) = entry
			.as_object()
			.and_then(|entry| entry.get("id"))
			.and_then(Value::as_str)
		{
			allowed.insert(id, entry);
		}
	}
	let mut personalized_catalog = Vec::with_capacity(catalog.len());
	for model in catalog {
		let AnyModel::Chat(chat) = model else {
			personalized_catalog.push(model.clone());
			continue;
		};
		if !is_model_type(model, ModelType::Chat) {
			personalized_catalog.push(model.clone());
			continue;
		}
		let Some(entry) = allowed.get(chat.id.as_str()) else {
			continue;
		};
		let entry = open_router_model(&chat.id, entry)?;
		let truthy = |value: Option<f64>| value.filter(|value| *value != 0.0 && !value.is_nan());
		let top_provider = entry.top_provider.as_ref();
		let mut personalized = chat.clone();
		// `||`: OpenRouter reports a missing limit as absent, null or 0, like the generator's fallback.
		personalized.context_window = truthy(top_provider.and_then(|top| top.context_length))
			.or(truthy(entry.context_length))
			.unwrap_or(chat.context_window);
		personalized.max_tokens =
			truthy(top_provider.and_then(|top| top.max_completion_tokens)).unwrap_or(chat.max_tokens);
		personalized.cost = match &entry.pricing {
			Some(pricing) => open_router_cost(Some(pricing)),
			None => chat.cost.clone(),
		};
		personalized_catalog.push(AnyModel::Chat(personalized));
	}
	Ok(personalized_catalog)
}

/// Runs `work` under `AbortSignal.timeout(ms)`: after `ms`, aborts `timeout` with a `TimeoutError` and keeps
/// awaiting `work`, which observes the abort through its signal.
async fn with_timeout<T>(work: impl Future<Output = T>, timeout: &AbortController, ms: f64) -> T {
	futures::pin_mut!(work);
	let sleep = pi_host::sleep(ms, None);
	futures::pin_mut!(sleep);
	match select(work, sleep).await {
		Either::Left((output, _)) => output,
		Either::Right((_, work)) => {
			timeout.abort_with(PiError::named(
				"TimeoutError",
				"The operation was aborted due to timeout",
			));
			work.await
		}
	}
}

/// `entry as OpenRouterModel` for an entry of `GET /models/user` with a string `id`.
fn open_router_model(id: &str, entry: &Value) -> PiResult<OpenRouterModel> {
	let number = |value: &Value, name: &str| value.get(name).and_then(Value::as_f64);
	let top_provider = entry.get("top_provider").filter(|top| top.is_object());
	Ok(OpenRouterModel {
		id: id.to_string(),
		context_length: number(entry, "context_length"),
		top_provider: top_provider.map(|top| OpenRouterTopProvider {
			context_length: number(top, "context_length"),
			max_completion_tokens: number(top, "max_completion_tokens"),
		}),
		pricing: entry
			.get("pricing")
			.filter(|pricing| is_truthy(pricing))
			.map(open_router_pricing)
			.transpose()?,
	})
}

fn is_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// A price as `perMillion` reads it: `String(value)` for `parseFloat`, with a missing or `null` value as missing.
fn price(value: &Value, name: &str) -> Option<String> {
	match value.get(name)? {
		Value::Null => None,
		price => Some(js_string(price)),
	}
}

/// `String(value)` for a JSON value.
fn js_string(value: &Value) -> String {
	match value {
		Value::Null => "null".into(),
		Value::Bool(value) => value.to_string(),
		Value::Number(value) => value.as_f64().map(pi_host::js::number_to_string).unwrap_or_default(),
		Value::String(value) => value.clone(),
		// `Array.prototype.join` prints `null` items as empty strings.
		Value::Array(items) => items
			.iter()
			.map(|item| if item.is_null() { String::new() } else { js_string(item) })
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

fn open_router_pricing(pricing: &Value) -> PiResult<OpenRouterPricing> {
	let overrides = match pricing.get("overrides") {
		None | Some(Value::Null) => None,
		Some(Value::Array(overrides)) => Some(overrides.iter().map(open_router_override).collect::<PiResult<_>>()?),
		Some(_) => {
			return Err(PiError::type_error(
				"((intermediate value) ?? []).flatMap is not a function",
			));
		}
	};
	Ok(OpenRouterPricing {
		prompt: price(pricing, "prompt"),
		completion: price(pricing, "completion"),
		input_cache_read: price(pricing, "input_cache_read"),
		input_cache_write: price(pricing, "input_cache_write"),
		overrides,
	})
}

fn open_router_override(price_override: &Value) -> PiResult<OpenRouterPricingOverride> {
	if price_override.is_null() {
		return Err(PiError::type_error(
			"Cannot read properties of null (reading 'min_prompt_tokens')",
		));
	}
	let present = |name: &str| price_override.get(name).is_some();
	Ok(OpenRouterPricingOverride {
		min_prompt_tokens: price_override.get("min_prompt_tokens").and_then(Value::as_f64),
		utc_start: present("utc_start").then_some(0.0),
		utc_end: present("utc_end").then_some(0.0),
		utc_days: present("utc_days").then(Vec::new),
		prompt: price(price_override, "prompt"),
		completion: price(price_override, "completion"),
		input_cache_read: price(price_override, "input_cache_read"),
		input_cache_write: price(price_override, "input_cache_write"),
	})
}

pub fn openrouter_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "openrouter".into(),
		name: Some("OpenRouter".into()),
		base_url: Some(OPENROUTER_BASE_URL.into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("OpenRouter API key", &["OPENROUTER_API_KEY"])),
			oauth: Some(lazy_oauth(LazyOAuthInput {
				name: "OpenRouter OAuth".into(),
				is_subscription: None,
				login_label: Some("Sign in with OpenRouter".into()),
				load: Rc::new(|| Box::pin(load_openrouter_oauth())),
			})),
		},
		models: OPENROUTER_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.chain(OPENROUTER_IMAGE_MODELS.values().cloned().map(AnyModel::Image))
			.chain(OPENROUTER_CLASSIFIER_MODELS.values().cloned().map(AnyModel::Classifier))
			.collect(),
		api: Some(ProviderApi::ByApi(IndexMap::from([
			("anthropic-messages".to_string(), anthropic_messages_api()),
			("openai-completions".to_string(), openai_completions_api()),
		]))),
		images: Some(IndexMap::from([(
			"openrouter-images".to_string(),
			openrouter_images_api(),
		)])),
		// OpenRouter serves TypeSafe's System One protocol at /api/v1/systemone.
		classifiers: Some(IndexMap::from([(
			"typesafe-system-one".to_string(),
			typesafe_system_one_api(),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
