//! Port of `packages/ai/test/max-thinking.test.ts`.
//!
//! `getModel`, `getSupportedThinkingLevels` and `clampThinkingLevel` (compat.ts) are `get_builtin_model` and the
//! `models.rs` functions. Not ported: `sends max to the Codex Responses API for %s`, which captures the request
//! through the openai-codex-responses API (wave-3, `w3-openai-responses`).

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use pi_ai::api::openai_codex_responses::stream_simple as stream_simple_openai_codex_responses;
use pi_ai::models::{clamp_thinking_level, get_supported_thinking_levels};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::providers::model_schema::{ModelThinkingLevel, ThinkingLevel, ThinkingLevelMap};
use pi_ai::types::{
	BaseModel, Context, Model, ModelCost, ModelInputModality, OnPayload, ProviderRequestOptions, SimpleStreamOptions,
	StreamOptions,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};

fn mock_token() -> String {
	let payload = base64::engine::general_purpose::STANDARD.encode(pi_host::js::json_stringify(
		&json!({ "https://api.openai.com/auth": { "chatgpt_account_id": "acc_test" } }),
	));
	format!("aaa.{payload}.bbb")
}

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual), Value::Number(expected)) => assert_eq!(actual.as_f64(), expected.as_f64()),
		_ => assert_eq!(actual, expected),
	}
}

use ModelThinkingLevel::{High, Low, Max, Medium, Minimal, Off, Xhigh};

fn reasoning_model(id: &str, name: &str, thinking_level_map: Option<ThinkingLevelMap>) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: name.into(),
			api: "openai-completions".into(),
			provider: "test".into(),
			base_url: "https://example.com/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: true,
		thinking_level_map,
		context_window: 128000.0,
		max_tokens: 4096.0,
		..Model::default()
	}
}

mod max_thinking_level {
	use super::*;

	#[test]
	fn is_opt_in_for_ordinary_reasoning_models() {
		let model = reasoning_model("ordinary-reasoning", "Ordinary Reasoning", None);

		assert_eq!(get_supported_thinking_levels(&model), [Off, Minimal, Low, Medium, High]);
		assert_eq!(clamp_thinking_level(&model, Max), High);
	}

	#[test]
	fn exposes_xhigh_and_max_for_openai_codex() {
		for model_id in [
			"gpt-5.6-luna",
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-6-luna",
			"gpt-6-sol",
		] {
			let model = get_builtin_model("openai-codex", model_id).expect("model is defined");
			assert_match_object(
				&serde_json::to_value(&model.thinking_level_map).unwrap(),
				&json!({ "xhigh": "xhigh", "max": "max" }),
			);
			assert_eq!(
				get_supported_thinking_levels(&model),
				[Off, Minimal, Low, Medium, High, Xhigh, Max],
				"{model_id}"
			);
		}
	}

	#[test]
	fn supports_a_hole_between_high_and_max() {
		let model = reasoning_model(
			"high-and-max",
			"High and Max",
			Some(serde_json::from_value(json!({ "xhigh": null, "max": "max" })).unwrap()),
		);

		assert_eq!(
			get_supported_thinking_levels(&model),
			[Off, Minimal, Low, Medium, High, Max]
		);
		assert_eq!(clamp_thinking_level(&model, Xhigh), Max);
	}

	#[test]
	fn sends_max_to_the_codex_responses_api() {
		for model_id in [
			"gpt-5.6-sol",
			"gpt-6-astra",
			"gpt-6-sol",
			"gpt-6-luna",
			"gpt-6.1-sol",
			"gpt-6.1-terra",
		] {
			pi_host::testing::run(|_host| async move {
				let model = get_builtin_model("openai-codex", model_id).expect("model is defined");
				let context = normalize_context(&Context {
					system_prompt: Some("You are a helpful assistant.".into()),
					messages: vec![
						serde_json::from_value(
							json!({ "role": "user", "content": "Hello", "timestamp": pi_host::now_ms() }),
						)
						.unwrap(),
					],
					tools: None,
				});
				let payload: Rc<RefCell<Option<Value>>> = Rc::default();
				let captured = payload.clone();
				let on_payload: OnPayload = Rc::new(
					move |request: Value, _model: &Model| -> LocalBoxFuture<'static, PiResult<Option<Value>>> {
						*captured.borrow_mut() = Some(request);
						Box::pin(async { Err(PiError::new("payload captured")) })
					},
				);

				let options = SimpleStreamOptions {
					base: StreamOptions {
						base: ProviderRequestOptions {
							api_key: Some(mock_token()),
							on_payload: Some(on_payload),
							..ProviderRequestOptions::default()
						},
						..StreamOptions::default()
					},
					reasoning: Some(ThinkingLevel::Max),
					..SimpleStreamOptions::default()
				};
				stream_simple_openai_codex_responses(&model, &context, Some(options))
					.unwrap()
					.result()
					.await;

				assert_match_object(
					payload.borrow().as_ref().expect("payload was captured"),
					&json!({ "reasoning": { "effort": "max", "summary": "auto" } }),
				);
			});
		}
	}
}
