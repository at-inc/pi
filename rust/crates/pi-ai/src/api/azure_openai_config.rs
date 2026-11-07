//! Port of `packages/ai/src/api/azure-openai-config.ts`.
//! PORT-STATUS: reviewed
//!
//! `AzureEndpointOptions extends StreamOptions` is `StreamOptions` plus the four `azure*` fields, reached through
//! `Deref`. URLs are parsed with the WHATWG-compliant `url` crate, the counterpart of `new URL`.

use std::ops::{Deref, DerefMut};

use indexmap::IndexMap;
use pi_host::{PiError, PiResult};
use url::Url;

use crate::types::{Model, StreamOptions};
use crate::utils::provider_env::get_provider_env_value;
use crate::utils::text::js_trim;

const DEFAULT_AZURE_API_VERSION: &str = "v1";

/// Azure models ship without a baseUrl: one resource per user, resolved per request.
#[derive(Clone, Default)]
pub struct AzureEndpointOptions {
	pub base: StreamOptions,
	pub azure_api_version: Option<String>,
	pub azure_resource_name: Option<String>,
	pub azure_base_url: Option<String>,
	pub azure_deployment_name: Option<String>,
}

impl Deref for AzureEndpointOptions {
	type Target = StreamOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for AzureEndpointOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

/// `resolveAzureConfig()`'s result.
#[derive(Clone, Debug, PartialEq)]
pub struct AzureConfig {
	pub base_url: String,
	pub api_version: String,
}

fn parse_deployment_name_map(value: Option<&str>) -> IndexMap<String, String> {
	let mut map = IndexMap::new();
	let Some(value) = value.filter(|value| !value.is_empty()) else {
		return map;
	};
	for entry in value.split(',') {
		let trimmed = js_trim(entry);
		if trimmed.is_empty() {
			continue;
		}
		// `split("=", 2)` keeps only the first two pieces.
		let mut parts = trimmed.split('=');
		let (Some(model_id), Some(deployment_name)) = (parts.next(), parts.next()) else {
			continue;
		};
		if model_id.is_empty() || deployment_name.is_empty() {
			continue;
		}
		map.insert(js_trim(model_id).to_string(), js_trim(deployment_name).to_string());
	}
	map
}

pub fn resolve_deployment_name(model: &Model, options: Option<&AzureEndpointOptions>) -> String {
	if let Some(name) = options
		.and_then(|options| options.azure_deployment_name.as_deref())
		.filter(|name| !name.is_empty())
	{
		return name.to_string();
	}
	let env = options.and_then(|options| options.env.as_ref());
	let mapped_deployment =
		parse_deployment_name_map(get_provider_env_value("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", env).as_deref())
			.get(&model.id)
			.cloned();
	mapped_deployment
		.filter(|name| !name.is_empty())
		.unwrap_or_else(|| model.id.clone())
}

fn trim_trailing_slashes(text: &str) -> &str {
	text.trim_end_matches('/')
}

fn normalize_azure_base_url(base_url: &str) -> PiResult<String> {
	let trimmed = trim_trailing_slashes(js_trim(base_url));
	let mut url =
		Url::parse(trimmed).map_err(|_| PiError::new(format!("Invalid Azure OpenAI base URL: {base_url}")))?;

	let hostname = url.host_str().unwrap_or("").to_string();
	let is_azure_host = hostname.ends_with(".openai.azure.com")
		|| hostname.ends_with(".cognitiveservices.azure.com")
		|| hostname.ends_with(".ai.azure.com");
	let normalized_path = trim_trailing_slashes(url.path()).to_string();

	// Ensure Azure hosts have /openai/v1 as base path so the AzureOpenAI SDK
	// can append /deployments/<model>/... and ?api-version=v1 correctly.
	if is_azure_host
		&& (normalized_path.is_empty()
			|| normalized_path == "/"
			|| normalized_path == "/openai"
			|| normalized_path == "/openai/v1/responses")
	{
		url.set_path("/openai/v1");
		url.set_query(None);
	}

	Ok(trim_trailing_slashes(url.as_str()).to_string())
}

fn build_default_base_url(resource_name: &str) -> String {
	format!("https://{resource_name}.openai.azure.com/openai/v1")
}

pub fn resolve_azure_base_url(model: &Model, options: Option<&AzureEndpointOptions>) -> PiResult<String> {
	let env = options.and_then(|options| options.env.as_ref());
	let base_url = options
		.and_then(|options| options.azure_base_url.as_deref())
		.map(js_trim)
		.filter(|value| !value.is_empty())
		.map(str::to_string)
		.or_else(|| {
			get_provider_env_value("AZURE_OPENAI_BASE_URL", env)
				.map(|value| js_trim(&value).to_string())
				.filter(|value| !value.is_empty())
		});
	let resource_name = options
		.and_then(|options| options.azure_resource_name.clone())
		.filter(|value| !value.is_empty())
		.or_else(|| get_provider_env_value("AZURE_OPENAI_RESOURCE_NAME", env));

	let mut resolved_base_url = base_url;

	if resolved_base_url.is_none()
		&& let Some(resource_name) = resource_name.filter(|value| !value.is_empty())
	{
		resolved_base_url = Some(build_default_base_url(&resource_name));
	}

	if resolved_base_url.is_none() && !model.base_url.is_empty() {
		resolved_base_url = Some(model.base_url.clone());
	}

	let Some(resolved_base_url) = resolved_base_url else {
		return Err(PiError::new(
			"Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME, or pass azureBaseUrl, azureResourceName, or model.baseUrl.",
		));
	};

	normalize_azure_base_url(&resolved_base_url)
}

pub fn resolve_azure_config(model: &Model, options: Option<&AzureEndpointOptions>) -> PiResult<AzureConfig> {
	let base_url = resolve_azure_base_url(model, options)?;
	let env = options.and_then(|options| options.env.as_ref());
	let api_version = options
		.and_then(|options| options.azure_api_version.clone())
		.filter(|value| !value.is_empty())
		.or_else(|| get_provider_env_value("AZURE_OPENAI_API_VERSION", env))
		.unwrap_or_else(|| DEFAULT_AZURE_API_VERSION.to_string());
	Ok(AzureConfig { base_url, api_version })
}
