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
//!   `ProviderStreamOptions.extra`, string values only. A non-string `azureBaseUrl` throws in the TS (`.trim` is not
//!   a function) and is ignored here.
//! - `SimpleStreamOptions` has no extra keys (types shard), so `streamSimple` cannot see `azureBaseUrl`,
//!   `azureDeploymentName` or `azureResourceName`: a `streamSimple` call that passes `azureBaseUrl` with no
//!   `AZURE_OPENAI_BASE_URL`/`AZURE_OPENAI_RESOURCE_NAME` in the env ends with the unconfigured-endpoint error, and a
//!   deployment name passed that way is not sent.
//! - `{ ...payload, model }` spreads an array payload into index keys and a string payload into one key per
//!   character, like JS; other primitives spread to no keys. A string with characters outside the BMP gets one key per
//!   character here where JS has one per UTF-16 code unit.
//! - The wrapped `onPayload` hands the caller's `onPayload` a copy of the params (the shared `OnPayload` signature
//!   takes the payload by value), so a caller that mutates the params in place and returns `undefined` loses the
//!   mutation; the TS sends it.

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
			Value::Array(items) => items
				.into_iter()
				.enumerate()
				.map(|(index, item)| (index.to_string(), item))
				.collect(),
			Value::String(text) => text
				.chars()
				.enumerate()
				.map(|(index, ch)| (index.to_string(), Value::String(ch.to_string())))
				.collect(),
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
			// `?? params`: an `undefined` or `null` result keeps the params.
			Ok(Some(replaced.filter(|replaced| !replaced.is_null()).unwrap_or(params)))
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
		Ok(lazy_stream(
			model,
			// The TS setup has no `await`, so resolution and the inner `stream()` call run inside `stream()`.
			|| {
				let endpoint = options
					.as_ref()
					.map(|options| azure_endpoint_options(&options.base, Some(&options.extra)));
				let inner = resolve_azure_model(model, endpoint.as_ref()).and_then(|resolved| {
					self.streams.stream(
						&resolved,
						context,
						with_deployment_name(model, options, endpoint.as_ref()),
					)
				});
				async move { inner }
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
		Ok(lazy_stream(
			model,
			// The TS setup has no `await`, so resolution and the inner `streamSimple()` call run inside `streamSimple()`.
			|| {
				let endpoint = options
					.as_ref()
					.map(|options| azure_endpoint_options(&options.base, None));
				let inner = resolve_azure_model(model, endpoint.as_ref()).and_then(|resolved| {
					self.streams.stream_simple(
						&resolved,
						context,
						with_deployment_name(model, options, endpoint.as_ref()),
					)
				});
				async move { inner }
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

#[cfg(test)]
mod tests {
	use std::cell::RefCell;

	use serde_json::json;

	use super::*;
	use crate::types::{BaseModel, Context, OnPayload};
	use crate::utils::transcript::normalize_context;

	#[derive(Default)]
	struct Captured {
		base_urls: Vec<String>,
		on_payload: Option<OnPayload>,
	}

	struct CapturingStreams {
		captured: Rc<RefCell<Captured>>,
	}

	impl ProviderStreams for CapturingStreams {
		fn stream(
			&self,
			model: &Model,
			_context: &TranscriptContext,
			options: Option<ProviderStreamOptions>,
		) -> PiResult<AssistantMessageEventStream> {
			let mut captured = self.captured.borrow_mut();
			captured.base_urls.push(model.base_url.clone());
			captured.on_payload = options.and_then(|options| options.on_payload.clone());
			let stream = AssistantMessageEventStream::new();
			stream.end(None);
			Ok(stream)
		}

		fn stream_simple(
			&self,
			_model: &Model,
			_context: &TranscriptContext,
			_options: Option<SimpleStreamOptions>,
		) -> PiResult<AssistantMessageEventStream> {
			unreachable!("these tests call stream()")
		}
	}

	fn capturing_azure_streams() -> (Rc<dyn ProviderStreams>, Rc<RefCell<Captured>>) {
		let captured = Rc::new(RefCell::new(Captured::default()));
		let streams = azure_streams(Rc::new(CapturingStreams {
			captured: captured.clone(),
		}));
		(streams, captured)
	}

	fn model() -> Model {
		Model {
			base: BaseModel {
				id: "gpt-test".into(),
				name: "gpt-test".into(),
				api: "openai-completions".into(),
				provider: "azure".into(),
				..BaseModel::default()
			},
			..Model::default()
		}
	}

	fn with_deployment(on_payload: Option<OnPayload>) -> ProviderStreamOptions {
		let mut options = ProviderStreamOptions {
			extra: serde_json::Map::from_iter([
				(
					"azureBaseUrl".to_string(),
					json!("https://example.openai.azure.com/openai/v1"),
				),
				("azureDeploymentName".to_string(), json!("deploy-1")),
			]),
			..ProviderStreamOptions::default()
		};
		options.on_payload = on_payload;
		options
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAzureBaseUrl (api/azure-openai-config.ts, w3-openai-responses)"]
	fn resolves_the_endpoint_when_stream_is_called() {
		pi_host::testing::run(|host| async move {
			let (streams, captured) = capturing_azure_streams();
			host.set_env("AZURE_OPENAI_BASE_URL", "https://example.openai.azure.com/openai/v1");
			let stream = streams
				.stream(&model(), &normalize_context(&Context::default()), None)
				.unwrap();
			host.set_env("AZURE_OPENAI_BASE_URL", "");
			assert_eq!(captured.borrow().base_urls.len(), 1);
			stream.result().await;
			assert_eq!(captured.borrow().base_urls.len(), 1);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAzureBaseUrl and resolveDeploymentName (api/azure-openai-config.ts, w3-openai-responses)"]
	fn a_null_on_payload_result_keeps_the_params() {
		pi_host::testing::run(|_| async {
			let (streams, captured) = capturing_azure_streams();
			let returns_null: OnPayload = Rc::new(|_, _| Box::pin(async { Ok(Some(Value::Null)) }));
			streams
				.stream(
					&model(),
					&normalize_context(&Context::default()),
					Some(with_deployment(Some(returns_null))),
				)
				.unwrap();
			let on_payload = captured.borrow().on_payload.clone().unwrap();
			assert_eq!(
				on_payload(json!({ "input": 1 }), &model()).await.unwrap(),
				Some(json!({ "input": 1, "model": "deploy-1" }))
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAzureBaseUrl and resolveDeploymentName (api/azure-openai-config.ts, w3-openai-responses)"]
	fn spreads_an_array_payload_into_index_keys() {
		pi_host::testing::run(|_| async {
			let (streams, captured) = capturing_azure_streams();
			streams
				.stream(
					&model(),
					&normalize_context(&Context::default()),
					Some(with_deployment(None)),
				)
				.unwrap();
			let on_payload = captured.borrow().on_payload.clone().unwrap();
			assert_eq!(
				on_payload(json!(["a", "b"]), &model()).await.unwrap(),
				Some(json!({ "0": "a", "1": "b", "model": "deploy-1" }))
			);
		});
	}
}
