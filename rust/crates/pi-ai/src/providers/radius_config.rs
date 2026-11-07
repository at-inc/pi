//! Port of `packages/ai/src/providers/radius-config.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `RadiusOAuthCredential` is `OAuthCredential`: its `gatewayConfig` lives in the credential's `extra` fields.
//! - A gateway model that passes `isRadiusGatewayModel` but does not deserialize as `RadiusGatewayModel` (for
//!   example an unknown input modality or a `cost` object without the rate fields), or whose `Model` built from it
//!   does not deserialize, is dropped; the TS keeps the raw object. Unknown keys of a gateway model survive in
//!   `extra` and become fields of the `Model`, like the TS spreads.
//! - `new URL("/v1/config", gateway)` throws `TypeError: Invalid URL` for an invalid gateway.

use pi_host::{AbortSignal, HttpRequest, PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::auth::types::OAuthCredential;
use crate::types::{Model, ModelCost, ModelInputModality, ThinkingLevelMap};
use crate::utils::text::{js_trim, owned_slice};

pub const DEFAULT_RADIUS_GATEWAY: &str = "https://radius.pi.dev";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiusGatewayModel {
	pub id: String,
	pub name: String,
	pub reasoning: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_level_map: Option<ThinkingLevelMap>,
	pub input: Vec<ModelInputModality>,
	pub cost: ModelCost,
	pub context_window: f64,
	pub max_tokens: f64,
	/// Fields beyond this type, kept like the TS object spread keeps them.
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadiusGatewayConfig {
	pub base_url: String,
	pub models: Vec<RadiusGatewayModel>,
}

/// `OAuthCredential & { gatewayConfig?: RadiusGatewayConfig }`; `gatewayConfig` is an `extra` field.
pub type RadiusOAuthCredential = OAuthCredential;

fn is_radius_gateway_model(value: &Value) -> bool {
	let Some(model) = value.as_object() else {
		return false;
	};
	model.get("id").is_some_and(Value::is_string)
		&& model.get("name").is_some_and(Value::is_string)
		&& model.get("reasoning").is_some_and(Value::is_boolean)
		&& model.get("input").is_some_and(Value::is_array)
		&& model.get("cost").is_some_and(Value::is_object)
		&& model.get("contextWindow").is_some_and(Value::is_number)
		&& model.get("maxTokens").is_some_and(Value::is_number)
}

fn sanitize_radius_gateway_config(config: Option<&Value>) -> Option<RadiusGatewayConfig> {
	let config = config?.as_object()?;
	let base_url = config.get("baseUrl")?.as_str()?;
	let models = config.get("models")?.as_array()?;
	Some(RadiusGatewayConfig {
		base_url: base_url.to_string(),
		models: models
			.iter()
			.filter(|model| is_radius_gateway_model(model))
			.filter_map(|model| serde_json::from_value(model.clone()).ok())
			.collect(),
	})
}

pub fn normalize_radius_gateway_url(value: &str) -> String {
	let bytes = value.as_bytes();
	let has_scheme = [b"http://".as_slice(), b"https://".as_slice()].iter().any(|scheme| {
		bytes
			.get(..scheme.len())
			.is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
	});
	let with_scheme = if has_scheme {
		value.to_string()
	} else {
		format!("https://{value}")
	};
	with_scheme.trim_end_matches('/').to_string()
}

pub fn get_radius_credential_config(credential: Option<&OAuthCredential>) -> Option<RadiusGatewayConfig> {
	sanitize_radius_gateway_config(credential.and_then(|credential| credential.extra.get("gatewayConfig")))
}

pub fn get_radius_models_from_config(provider_id: &str, config: &RadiusGatewayConfig) -> Vec<Model> {
	config
		.models
		.iter()
		.filter_map(|model| {
			let Ok(Value::Object(mut fields)) = serde_json::to_value(model) else {
				return None;
			};
			fields.insert("api".into(), Value::String("pi-messages".into()));
			fields.insert("provider".into(), Value::String(provider_id.into()));
			fields.insert("baseUrl".into(), Value::String(config.base_url.clone()));
			serde_json::from_value(Value::Object(fields)).ok()
		})
		.collect()
}

pub fn get_radius_models(provider_id: &str, credential: Option<&OAuthCredential>) -> Vec<Model> {
	match get_radius_credential_config(credential) {
		Some(config) => get_radius_models_from_config(provider_id, &config),
		None => Vec::new(),
	}
}

fn truncate_http_body(body: &str) -> String {
	let trimmed = js_trim(body);
	if pi_host::js::utf16_len(trimmed) > 512 {
		format!("{}…", owned_slice(trimmed, 0, Some(512)))
	} else {
		trimmed.to_string()
	}
}

pub async fn load_radius_gateway_config(
	gateway: &str,
	api_key: Option<&str>,
	signal: Option<AbortSignal>,
) -> PiResult<RadiusGatewayConfig> {
	let url = url::Url::parse(gateway)
		.and_then(|base| base.join("/v1/config"))
		.map_err(|_| PiError::type_error("Invalid URL"))?;
	let mut request = HttpRequest::new("GET", url.as_str());
	request.set_header("accept", "application/json");
	if let Some(api_key) = api_key.filter(|api_key| !api_key.is_empty()) {
		request.set_header("authorization", format!("Bearer {api_key}"));
	}
	request.signal = signal;
	let response = pi_host::fetch(request, None).await?;
	if !response.ok() {
		let status = response.status;
		return Err(PiError::new(format!(
			"Could not load Radius config from {gateway}: {status}: {}",
			truncate_http_body(&response.text().await?)
		)));
	}
	let config = sanitize_radius_gateway_config(Some(&response.json().await?));
	config.ok_or_else(|| PiError::new(format!("Invalid Radius config from {gateway}")))
}
