//! Port of `packages/ai/test/openai-completions-thinking-token-budget.test.ts`.

mod openai_completions_support;

use openai_completions_support::{capture_payload, model, serve_chunks, user_message};
use pi_ai::compat::stream_simple;
use pi_ai::types::{Context, Model, SimpleStreamOptions, ThinkingBudgets, ThinkingLevel};
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

fn vllm_model(compat: Value) -> Model {
	let mut model = model("zai-org/glm-5.2", "local-vllm", "http://localhost:8000/v1");
	model.base.name = "GLM 5.2 (local vLLM)".into();
	model.reasoning = true;
	model.context_window = 262_144.0;
	model.max_tokens = 16384.0;
	model.compat = compat.as_object().cloned();
	model
}

fn default_compat() -> Value {
	json!({ "thinkingFormat": "zai", "supportsThinkingTokenBudget": true })
}

async fn capture(
	host: &TestHost,
	model: Model,
	reasoning: Option<ThinkingLevel>,
	thinking_budgets: Option<ThinkingBudgets>,
	max_tokens: Option<f64>,
) -> Value {
	serve_chunks(
		host,
		vec![json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] })],
	);
	let (on_payload, payload) = capture_payload(None);
	let mut options = SimpleStreamOptions::default();
	options.base.api_key = Some("test".into());
	options.reasoning = reasoning;
	options.thinking_budgets = thinking_budgets;
	options.max_tokens = max_tokens;
	options.on_payload = Some(on_payload);
	stream_simple(
		&model,
		&Context {
			messages: vec![user_message("Hi")],
			..Context::default()
		},
		Some(options),
	)
	.unwrap()
	.result()
	.await;
	payload.borrow().clone().expect("payload")
}

fn budgets(medium: Option<f64>, high: Option<f64>) -> Option<ThinkingBudgets> {
	Some(ThinkingBudgets {
		medium,
		high,
		..ThinkingBudgets::default()
	})
}

mod openai_completions_thinking_token_budget {
	use super::*;

	#[test]
	fn sends_the_configured_budget_for_the_requested_level() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(default_compat()),
				Some(ThinkingLevel::Medium),
				budgets(Some(4096.0), None),
				None,
			)
			.await;
			assert_eq!(params["thinking_token_budget"], json!(4096));
		});
	}

	#[test]
	fn omits_the_budget_when_neither_the_field_nor_the_alias_is_set() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(json!({ "thinkingFormat": "zai" })),
				Some(ThinkingLevel::Medium),
				budgets(Some(4096.0), None),
				None,
			)
			.await;
			assert_eq!(params.get("thinking_token_budget"), None);
			assert_eq!(params.get("thinking_budget"), None);
			assert_eq!(params.get("thinking_budget_tokens"), None);
		});
	}

	#[test]
	fn omits_the_budget_when_thinking_is_off() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(default_compat()),
				None,
				budgets(None, Some(8192.0)),
				None,
			)
			.await;
			assert_eq!(params.get("thinking_token_budget"), None);
		});
	}

	#[test]
	fn clamps_xhigh_and_max_to_the_high_budget() {
		run(|host| async move {
			let xhigh = capture(
				&host,
				vllm_model(default_compat()),
				Some(ThinkingLevel::Xhigh),
				budgets(None, Some(8192.0)),
				None,
			)
			.await;
			let max = capture(
				&host,
				vllm_model(default_compat()),
				Some(ThinkingLevel::Max),
				budgets(None, Some(8192.0)),
				None,
			)
			.await;
			assert_eq!(xhigh["thinking_token_budget"], json!(8192));
			assert_eq!(max["thinking_token_budget"], json!(8192));
		});
	}

	#[test]
	fn leaves_room_for_the_answer_when_the_budget_meets_the_response_ceiling() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(default_compat()),
				Some(ThinkingLevel::High),
				None,
				None,
			)
			.await;
			assert_eq!(params["thinking_token_budget"], json!(16384 - 1024));
		});
	}

	#[test]
	fn uses_the_caller_max_tokens_as_the_ceiling_when_it_is_lower_than_the_model_cap() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(default_compat()),
				Some(ThinkingLevel::High),
				budgets(None, Some(8192.0)),
				Some(4096.0),
			)
			.await;
			assert_eq!(params["thinking_token_budget"], json!(4096 - 1024));
		});
	}

	#[test]
	fn sends_the_field_when_thinking_token_budget_field_is_set() {
		run(|host| async move {
			for field in ["thinking_budget", "thinking_budget_tokens"] {
				let params = capture(
					&host,
					vllm_model(json!({ "thinkingFormat": "qwen", "thinkingTokenBudgetField": field })),
					Some(ThinkingLevel::Medium),
					budgets(Some(4096.0), None),
					None,
				)
				.await;
				assert_eq!(params[field], json!(4096), "{field}");
				assert_eq!(params.get("thinking_token_budget"), None, "{field}");
			}
		});
	}

	#[test]
	fn lets_thinking_token_budget_field_win_over_the_boolean_alias() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(json!({
					"thinkingFormat": "zai",
					"supportsThinkingTokenBudget": true,
					"thinkingTokenBudgetField": "thinking_budget",
				})),
				Some(ThinkingLevel::Medium),
				budgets(Some(4096.0), None),
				None,
			)
			.await;
			assert_eq!(params["thinking_budget"], json!(4096));
			assert_eq!(params.get("thinking_token_budget"), None);
		});
	}

	#[test]
	fn puts_the_clamped_budget_in_chat_template_kwargs_when_var_is_thinking_budget() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(json!({
					"thinkingFormat": "chat-template",
					"chatTemplateKwargs": {
						"enable_thinking": { "$var": "thinking.enabled" },
						"thinking_budget": { "$var": "thinking.budget" },
					},
				})),
				Some(ThinkingLevel::High),
				None,
				None,
			)
			.await;
			assert_eq!(
				params["chat_template_kwargs"],
				json!({ "enable_thinking": true, "thinking_budget": 16384 - 1024 })
			);
			assert_eq!(params.get("thinking_token_budget"), None);
		});
	}

	#[test]
	fn omits_thinking_budget_from_chat_template_kwargs_when_thinking_is_off() {
		run(|host| async move {
			let params = capture(
				&host,
				vllm_model(json!({
					"thinkingFormat": "chat-template",
					"chatTemplateKwargs": {
						"enable_thinking": { "$var": "thinking.enabled" },
						"thinking_budget": { "$var": "thinking.budget" },
					},
				})),
				None,
				None,
				None,
			)
			.await;
			assert_eq!(params["chat_template_kwargs"], json!({ "enable_thinking": false }));
		});
	}
}
