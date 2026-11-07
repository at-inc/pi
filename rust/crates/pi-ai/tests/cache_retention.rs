//! Port of `packages/ai/test/cache-retention.test.ts`: the "OpenAI Responses Provider" describe.
//!
//! Not ported here: the "Anthropic Provider" describe (`api::anthropic_messages`, w3-anthropic) and the "OpenAI
//! Completions Provider" describe (`api::openai_completions`, w3-openai-completions); both adapters are still
//! `PORT-STATUS: pending` on this branch, and their shards add those modules to this file. The two
//! `skipIf(!process.env.OPENAI_API_KEY)` cases are e2e. `PI_CACHE_RETENTION` is read from the test host's environment,
//! which starts empty like the `beforeEach` that deletes it.

mod openai_responses_support;

use std::rc::Rc;

use openai_responses_support::{context, responses_options, with};
use pi_ai::api::openai_responses::{OpenAIResponsesOptions, stream as stream_open_ai_responses};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{AssistantMessageEvent, CacheRetention, Model, OnPayload, TranscriptContext};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

fn get_model(provider: &str, model_id: &str) -> Model {
	get_builtin_model(provider, model_id).unwrap_or_else(|| panic!("{provider}/{model_id} is defined"))
}

fn hello_context() -> TranscriptContext {
	context(json!({
		"systemPrompt": "You are a helpful assistant.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": pi_host::now_ms() }],
	}))
}

/// Streams until the first error with `onPayload: stopAfterPayload(...)` and returns the captured payload.
async fn capture(model: &Model, configure: impl FnOnce(&mut OpenAIResponsesOptions)) -> Value {
	let captured = Rc::new(std::cell::RefCell::new(None));
	let target = captured.clone();
	let on_payload: OnPayload = Rc::new(move |payload, _model| {
		*target.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::named("PayloadCaptured", "payload captured")) })
	});
	let options = responses_options(|options| {
		options.api_key = Some("sk-fake-key".into());
		options.on_payload = Some(on_payload);
		configure(options);
	});
	let stream = stream_open_ai_responses(model, &hello_context(), Some(options));
	while let Some(event) = stream.next().await {
		if matches!(event, AssistantMessageEvent::Error { .. }) {
			break;
		}
	}
	captured.borrow_mut().take().expect("payload was captured")
}

mod cache_retention_pi_cache_retention {
	use super::*;

	mod open_ai_responses_provider {
		use super::*;

		#[test]
		fn does_not_enable_cache_warming_from_the_documented_ttl_alone() {
			for model_id in [
				"gpt-5.6-luna",
				"gpt-5.6-sol",
				"gpt-5.6-terra",
				"gpt-6-astra",
				"gpt-6-luna",
				"gpt-6-sol",
				"gpt-6.1-terra",
			] {
				assert!(get_model("openai", model_id).prompt_cache.is_none(), "{model_id}");
			}
		}

		#[test]
		fn should_set_prompt_cache_retention_for_non_api_openai_com_base_url_by_default() {
			run(|host| async move {
				host.set_env("PI_CACHE_RETENTION", "long");

				// Create a model with a different baseUrl (simulating a proxy)
				let proxy_model = with(
					&get_model("openai", "gpt-4o-mini"),
					json!({ "baseUrl": "https://my-proxy.example.com/v1" }),
				);
				let payload = capture(&proxy_model, |_| {}).await;

				assert_eq!(payload["prompt_cache_retention"], "24h");
			});
		}

		#[test]
		fn should_omit_prompt_cache_retention_when_supports_long_cache_retention_is_false() {
			run(|_| async {
				let model = with(
					&get_model("openai", "gpt-4o-mini"),
					json!({ "compat": { "supportsLongCacheRetention": false } }),
				);
				let payload = capture(&model, |options| {
					options.cache_retention = Some(CacheRetention::Long);
					options.session_id = Some("session-compat-false".into());
				})
				.await;

				assert!(payload.get("prompt_cache_retention").is_none());
			});
		}

		#[test]
		fn should_omit_prompt_cache_key_and_disable_implicit_writes_when_cache_retention_is_none() {
			run(|_| async {
				let payload = capture(&get_model("openai", "gpt-5.6-sol"), |options| {
					options.cache_retention = Some(CacheRetention::None);
					options.session_id = Some("session-1".into());
				})
				.await;

				assert!(payload.get("prompt_cache_key").is_none());
				assert!(payload.get("prompt_cache_retention").is_none());
				assert_eq!(payload["prompt_cache_options"], json!({ "mode": "explicit" }));
			});
		}

		#[test]
		fn should_omit_prompt_cache_options_for_models_that_reject_it() {
			run(|_| async {
				let payload = capture(&get_model("openai", "gpt-4o-mini"), |options| {
					options.cache_retention = Some(CacheRetention::None);
					options.session_id = Some("session-1".into());
				})
				.await;

				assert!(payload.get("prompt_cache_key").is_none());
				assert!(payload.get("prompt_cache_options").is_none());
			});
		}

		#[test]
		fn should_use_the_supported_long_cache_field() {
			let cases = [
				("gpt-4o-mini", Some("24h"), None),
				("gpt-6-astra", None, Some(json!({ "ttl": "30m" }))),
				("gpt-6-sol", None, Some(json!({ "ttl": "30m" }))),
				("gpt-6-luna", None, Some(json!({ "ttl": "30m" }))),
			];
			for (model_id, retention, cache_options) in cases {
				run(|_| async move {
					let payload = capture(&get_model("openai", model_id), |options| {
						options.cache_retention = Some(CacheRetention::Long);
						options.session_id = Some("session-2".into());
					})
					.await;

					assert_eq!(payload["prompt_cache_key"], "session-2", "{model_id}");
					assert_eq!(
						payload.get("prompt_cache_retention").and_then(Value::as_str),
						retention,
						"{model_id}"
					);
					assert_eq!(
						payload.get("prompt_cache_options").cloned(),
						cache_options,
						"{model_id}"
					);
				});
			}
		}
	}
}
