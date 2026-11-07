//! Port of the Anthropic cases of `packages/ai/test/cache-retention.test.ts`.
//!
//! The "OpenAI Responses Provider" and "OpenAI Completions Provider" describes move with those APIs (w3 OpenAI
//! shards). The two `it.skipIf(!process.env.ANTHROPIC_API_KEY)` cases go through `stream` with a real key and
//! are e2e; they are not ported.

mod anthropic_support;

use std::cell::RefCell;
use std::rc::Rc;

use anthropic_support::{context, model};
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::{CacheRetention, Model, StreamOptions, TranscriptContext};
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
}
