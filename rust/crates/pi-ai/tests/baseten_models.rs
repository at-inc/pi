//! Port of `packages/ai/test/baseten-models.test.ts`.
//!
//! `getModel` (compat.ts) is `get_builtin_model`. `process.env` is the `TestHost` environment, so the `afterEach`
//! restore is not needed.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::stream_simple;
use pi_ai::env_api_keys::{find_env_keys, get_env_api_key};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{Context, ModelInputModality, SimpleStreamOptions, ThinkingLevel};
use pi_host::PiError;
use serde_json::{Value, json};

/// `streamSimple(model, { messages: [test] }, { apiKey, reasoning, onPayload })`: the payload `onPayload` captured
/// before it threw.
async fn capture_payload(reasoning: Option<ThinkingLevel>) -> Value {
	let model = get_builtin_model("baseten", "zai-org/GLM-5.2").unwrap();
	let context: Context =
		serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "test", "timestamp": 0 }] })).unwrap();
	let captured: Rc<RefCell<Option<Value>>> = Rc::default();
	let recorder = captured.clone();
	let mut options = SimpleStreamOptions {
		reasoning,
		..SimpleStreamOptions::default()
	};
	options.api_key = Some("test-baseten-key".into());
	options.on_payload = Some(Rc::new(move |payload, _| {
		*recorder.borrow_mut() = Some(payload);
		Box::pin(async { Err(PiError::new("payload captured")) })
	}));
	stream_simple(&model, &context, Some(options)).unwrap().result().await;
	captured.borrow_mut().take().expect("payload captured")
}

mod baseten_models {
	use super::*;

	#[test]
	fn keeps_both_glm_5_2_endpoints_text_only() {
		for id in ["zai-org/GLM-5.2", "zai-org/GLM-5.2-Fast"] {
			assert_eq!(
				get_builtin_model("baseten", id).unwrap().input,
				[ModelInputModality::Text],
				"{id}"
			);
		}
	}

	#[test]
	fn sends_baseten_chat_template_args_with_reasoning_effort() {
		pi_host::testing::run(|_| async {
			let payload = capture_payload(Some(ThinkingLevel::High)).await;

			assert_eq!(payload["chat_template_args"], json!({ "enable_thinking": true }));
			assert_eq!(payload["reasoning_effort"], "high");
		});
	}

	#[test]
	fn disables_baseten_opt_in_reasoning_when_thinking_is_off() {
		pi_host::testing::run(|_| async {
			let payload = capture_payload(None).await;

			assert_eq!(payload["chat_template_args"], json!({ "enable_thinking": false }));
			assert_eq!(payload["reasoning_effort"], "none");
		});
	}

	#[test]
	fn resolves_baseten_api_key_from_the_environment() {
		pi_host::testing::run(|host| async move {
			host.set_env("BASETEN_API_KEY", "test-baseten-key");

			assert_eq!(
				find_env_keys("baseten", None),
				Some(vec!["BASETEN_API_KEY".to_string()])
			);
			assert_eq!(get_env_api_key("baseten", None).as_deref(), Some("test-baseten-key"));
		});
	}
}
