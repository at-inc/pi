//! Port of `packages/ai/src/providers/azure.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.
//! - `azureStreams`' wrapper is a private struct implementing `ProviderStreams`. `withDeploymentName<T>` is generic over
//!   the two stream option types, which both deref to `StreamOptions`.
//! - `resolveAzureBaseUrl` / `resolveDeploymentName` take `AzureEndpointOptions`: its `azure*` fields are read from
//!   `ProviderStreamOptions.extra` (string values only); `SimpleStreamOptions` has no extra keys, so simple requests
//!   resolve Azure settings from the provider env only.
//! - `{ ...payload, model }` of a non-object payload is `{ model }` (JS spreads a primitive to no keys).

use std::ops::DerefMut;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::PiResult;
use serde_json::Value;

use crate::api::azure_openai_config::{AzureEndpointOptions, resolve_azure_base_url, resolve_deployment_name};
use crate::api::azure_openai_responses_lazy::azure_openai_responses_api;
use crate::api::lazy::lazy_stream;
use crate::api::openai_completions_lazy::openai_completions_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::azure_models::AZURE_MODELS;
use crate::types::{
	AnyModel, Model, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions, StreamOptions, TranscriptContext,
};
use crate::utils::event_stream::AssistantMessageEventStream;

/// `AzureEndpointOptions` view of request options: the base options plus the `azure*` keys found in `extra`.
fn azure_endpoint_options(
	base: &StreamOptions,
	extra: Option<&serde_json::Map<String, Value>>,
) -> AzureEndpointOptions {
	let field = |name: &str| {
		extra
			.and_then(|extra| extra.get(name))
			.and_then(Value::as_str)
			.map(str::to_string)
	};
	AzureEndpointOptions {
		base: base.clone(),
		azure_api_version: field("azureApiVersion"),
		azure_resource_name: field("azureResourceName"),
		azure_base_url: field("azureBaseUrl"),
		azure_deployment_name: field("azureDeploymentName"),
	}
}

fn resolve_azure_model(model: &Model, options: Option<&AzureEndpointOptions>) -> PiResult<Model> {
	let mut resolved = model.clone();
	resolved.base_url = resolve_azure_base_url(model, options)?;
	Ok(resolved)
}

/// Send the deployment name as the request's model, keeping `model.id` as the catalog id.
fn with_deployment_name<T>(model: &Model, options: Option<T>, endpoint: Option<&AzureEndpointOptions>) -> Option<T>
where
	T: Default + DerefMut<Target = StreamOptions>,
{
	let deployment_name = resolve_deployment_name(model, endpoint);
	if deployment_name == model.id {
		return options;
	}
	let mut options = options;
	let inner = options.as_ref().and_then(|options| options.on_payload.clone());
	let mut wrapped = options.take().unwrap_or_default();
	wrapped.on_payload = Some(Rc::new(move |payload: Value, payload_model: &Model| {
		let mut params = match payload {
			Value::Object(payload) => payload,
			_ => serde_json::Map::new(),
		};
		params.insert("model".into(), Value::String(deployment_name.clone()));
		let params = Value::Object(params);
		let (inner, payload_model) = (inner.clone(), payload_model.clone());
		Box::pin(async move {
			let replaced = match inner {
				Some(inner) => inner(params.clone(), &payload_model).await?,
				None => None,
			};
			Ok(Some(replaced.unwrap_or(params)))
		})
	}));
	Some(wrapped)
}

/// Resolve the Azure endpoint and deployment before dispatch, inside `lazyStream` so an
/// unconfigured endpoint errors on the stream instead of throwing out of `stream()`.
struct AzureStreams {
	streams: Rc<dyn ProviderStreams>,
}

impl ProviderStreams for AzureStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let (streams, request_model, context) = (self.streams.clone(), model.clone(), context.clone());
		Ok(lazy_stream(
			model,
			move || async move {
				let endpoint = options
					.as_ref()
					.map(|options| azure_endpoint_options(&options.base, Some(&options.extra)));
				let resolved = resolve_azure_model(&request_model, endpoint.as_ref())?;
				streams.stream(
					&resolved,
					&context,
					with_deployment_name(&request_model, options, endpoint.as_ref()),
				)
			},
			None,
		))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let (streams, request_model, context) = (self.streams.clone(), model.clone(), context.clone());
		Ok(lazy_stream(
			model,
			move || async move {
				let endpoint = options
					.as_ref()
					.map(|options| azure_endpoint_options(&options.base, None));
				let resolved = resolve_azure_model(&request_model, endpoint.as_ref())?;
				streams.stream_simple(
					&resolved,
					&context,
					with_deployment_name(&request_model, options, endpoint.as_ref()),
				)
			},
			None,
		))
	}
}

fn azure_streams(streams: Rc<dyn ProviderStreams>) -> Rc<dyn ProviderStreams> {
	Rc::new(AzureStreams { streams })
}

pub fn azure_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "azure".into(),
		name: Some("Azure".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("Azure OpenAI API key", &["AZURE_OPENAI_API_KEY"])),
			oauth: None,
		},
		models: AZURE_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::ByApi(IndexMap::from([
			("azure-openai-responses".to_string(), azure_openai_responses_api()),
			(
				"openai-completions".to_string(),
				azure_streams(openai_completions_api()),
			),
		]))),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
