//! Port of `packages/ai/test/cache-retention.test.ts`.
//!
//! `MODELS.<provider>[id]` is `get_builtin_model`; `it.each` cases run in a loop. The cerebras `contextWithTools`
//! literal is passed through `normalizeContext`, which leaves an already normalized transcript as it is. The
//! `it.skipIf(!process.env.ANTHROPIC_API_KEY)` and `skipIf(!process.env.OPENAI_API_KEY)` cases go through `stream`
//! with a real key and are e2e; they are not ported. `PI_CACHE_RETENTION` is read from the test host's environment,
//! which starts empty like the `beforeEach` that deletes it.

mod anthropic_support;
mod openai_completions_support;
mod openai_responses_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::{context, model};
use openai_completions_support::model as completions_model;
use openai_responses_support::{context as responses_context, responses_options, with};
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::api::openai_completions::{OpenAICompletionsOptions, stream as stream_open_ai_completions};
use pi_ai::api::openai_responses::{OpenAIResponsesOptions, stream as stream_open_ai_responses};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{
	AssistantMessageEvent, CacheRetention, Model, OnPayload, ProviderRequestOptions, StreamOptions, TranscriptContext,
};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

fn retention_context() -> TranscriptContext {
	context(json!({
		"systemPrompt": "You are a helpful assistant.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }],
	}))
}

/// Streams `model` with a fake key and a `stopAfterPayload` hook, and returns the captured payload.
async fn capture_payload(model: &Model, cache_retention: Option<CacheRetention>) -> Value {
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let sink = captured.clone();
	let mut base = StreamOptions {
		cache_retention,
		..StreamOptions::default()
	};
	base.api_key = Some("fake-key".into());
	base.on_payload = Some(Rc::new(move |payload, _| {
		*sink.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::new("payload captured")) })
	}));
	let options = AnthropicOptions {
		base,
		..AnthropicOptions::default()
	};
	let _ = stream(model, &retention_context(), Some(options)).result().await;
	captured.borrow_mut().take().expect("payload captured")
}

fn proxy_model(compat: Option<Value>) -> Model {
	let mut model = model("anthropic", "claude-haiku-4-5");
	model.base_url = "https://my-proxy.example.com/v1".into();
	if let Some(Value::Object(compat)) = compat {
		model.compat = Some(compat);
	}
	model
}

fn get_model(provider: &str, model_id: &str) -> Model {
	get_builtin_model(provider, model_id).unwrap_or_else(|| panic!("{provider}/{model_id} is defined"))
}

fn hello_context() -> TranscriptContext {
	responses_context(json!({
		"systemPrompt": "You are a helpful assistant.",
		"messages": [{ "role": "user", "content": "Hello", "timestamp": pi_host::now_ms() }],
	}))
}

/// Streams until the first error with `onPayload: stopAfterPayload(...)` and returns the captured payload.
async fn capture(model: &Model, configure: impl FnOnce(&mut OpenAIResponsesOptions)) -> Value {
	let captured = Rc::new(RefCell::new(None));
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

	mod anthropic_provider {
		use super::*;

		#[test]
		fn should_add_ttl_for_non_api_anthropic_com_base_url_by_default() {
			run(|host| {
				host.set_env("PI_CACHE_RETENTION", "long");
				async {
					let payload = capture_payload(&proxy_model(None), None).await;
					assert_eq!(
						payload["system"][0]["cache_control"],
						json!({ "type": "ephemeral", "ttl": "1h" })
					);
				}
			});
		}

		#[test]
		fn should_omit_ttl_when_supports_long_cache_retention_is_false() {
			run(|_| async {
				let model = proxy_model(Some(json!({ "supportsLongCacheRetention": false })));
				let payload = capture_payload(&model, Some(CacheRetention::Long)).await;
				assert_eq!(payload["system"][0]["cache_control"], json!({ "type": "ephemeral" }));
			});
		}

		#[test]
		fn should_omit_cache_control_when_cache_retention_is_none() {
			run(|_| async {
				let model = model("anthropic", "claude-haiku-4-5");
				let payload = capture_payload(&model, Some(CacheRetention::None)).await;
				assert!(payload["system"][0].get("cache_control").is_none());
			});
		}

		#[test]
		fn should_add_cache_control_to_string_user_messages() {
			run(|_| async {
				let model = model("anthropic", "claude-haiku-4-5");
				let payload = capture_payload(&model, None).await;
				let messages = payload["messages"].as_array().unwrap();
				let content = messages.last().unwrap()["content"]
					.as_array()
					.expect("content is an array");
				assert_eq!(content.last().unwrap()["cache_control"], json!({ "type": "ephemeral" }));
			});
		}

		#[test]
		fn should_set_1h_cache_ttl_when_cache_retention_is_long() {
			run(|_| async {
				let model = model("anthropic", "claude-haiku-4-5");
				let payload = capture_payload(&model, Some(CacheRetention::Long)).await;
				assert_eq!(
					payload["system"][0]["cache_control"],
					json!({ "type": "ephemeral", "ttl": "1h" })
				);
			});
		}
	}

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

	mod open_ai_completions_provider {
		use super::*;

		fn create_completions_model(compat: Option<Value>) -> Model {
			let mut model = completions_model(
				"test-model",
				"test-openai-completions",
				"https://my-proxy.example.com/v1",
			);
			model.name = "Test Model".into();
			if let Some(Value::Object(compat)) = compat {
				model.compat = Some(compat);
			}
			model
		}

		/// Streams through `openai-completions` until the first error and returns the captured payload.
		async fn capture_completions(
			model: &Model,
			context: &TranscriptContext,
			configure: impl FnOnce(&mut OpenAICompletionsOptions),
		) -> Value {
			let captured = Rc::new(RefCell::new(None));
			let target = captured.clone();
			let mut options = OpenAICompletionsOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some("fake-key".into()),
						on_payload: Some(Rc::new(move |payload, _model| {
							*target.borrow_mut() = Some(payload);
							Box::pin(async { Err(PiError::named("PayloadCaptured", "payload captured")) })
						})),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				},
				..OpenAICompletionsOptions::default()
			};
			configure(&mut options);
			let stream = stream_open_ai_completions(model, context, Some(options));
			while let Some(event) = stream.next().await {
				if matches!(event, AssistantMessageEvent::Error { .. }) {
					break;
				}
			}
			captured.borrow_mut().take().expect("payload was captured")
		}

		#[test]
		fn should_set_prompt_cache_retention_for_non_api_openai_com_base_url_by_default() {
			run(|_| async {
				let payload = capture_completions(&create_completions_model(None), &hello_context(), |options| {
					options.cache_retention = Some(CacheRetention::Long);
					options.session_id = Some("session-completions".into());
				})
				.await;

				assert_eq!(payload["prompt_cache_key"], "session-completions");
				assert_eq!(payload["prompt_cache_retention"], "24h");
			});
		}

		#[test]
		fn should_omit_prompt_cache_retention_when_supports_long_cache_retention_is_false() {
			run(|_| async {
				let model = create_completions_model(Some(json!({ "supportsLongCacheRetention": false })));
				let payload = capture_completions(&model, &hello_context(), |options| {
					options.cache_retention = Some(CacheRetention::Long);
					options.session_id = Some("session-completions-false".into());
				})
				.await;

				assert!(payload.get("prompt_cache_key").is_none());
				assert!(payload.get("prompt_cache_retention").is_none());
			});
		}

		#[test]
		fn should_omit_long_cache_retention_for_provider_id() {
			for model_id in [
				"deepseek-v4.1-flash",
				"deepseek-v4-pro",
				"kimi-k2.5",
				"kimi-k2.6",
				"minimax-m2.7",
			] {
				run(|_| async move {
					let model = get_model("opencode", model_id);
					let payload = capture_completions(&model, &hello_context(), |options| {
						options.cache_retention = Some(CacheRetention::Long);
						options.session_id = Some("session-opencode-long-cache-unsupported".into());
					})
					.await;

					assert_eq!(
						model
							.compat
							.as_ref()
							.and_then(|compat| compat.get("supportsLongCacheRetention")),
						Some(&json!(false)),
						"opencode/{model_id}"
					);
					assert!(payload.get("prompt_cache_key").is_none(), "opencode/{model_id}");
					assert!(payload.get("prompt_cache_retention").is_none(), "opencode/{model_id}");
				});
			}
		}

		#[test]
		fn should_omit_strict_field_on_tools_for_cerebras_id() {
			for model_id in ["gpt-oss-120b", "qwen-3.8-27b"] {
				run(|_| async move {
					let model = get_model("cerebras", model_id);
					let context_with_tools = responses_context(json!({
						"messages": [
							{
								"role": "system",
								"content": "test",
								"toolsAdded": [
									{
										"name": "t1",
										"description": "strict tool",
										"parameters": {
											"type": "object",
											"properties": { "x": { "type": "string" } },
											"required": ["x"],
										},
										"constrainedSampling": { "type": "json_schema" },
									},
									{
										"name": "t2",
										"description": "non-strict tool",
										"parameters": {
											"type": "object",
											"properties": { "y": { "type": "string" } },
											"required": ["y"],
										},
									},
								],
								"timestamp": 0,
							},
							{ "role": "user", "content": "hello", "timestamp": 1 },
						],
					}));
					let payload = capture_completions(&model, &context_with_tools, |options| {
						options.session_id = Some("test".into());
					})
					.await;

					assert!(
						model
							.compat
							.as_ref()
							.and_then(|compat| compat.get("supportsStrictMode"))
							.is_none(),
						"cerebras/{model_id}"
					);
					let tools = payload["tools"].as_array().expect("tools are defined");
					for tool in tools {
						assert!(tool["function"].get("strict").is_none(), "cerebras/{model_id}");
					}
				});
			}
		}
	}
}
