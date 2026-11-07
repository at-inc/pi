//! Port of `packages/ai/test/context-estimate.test.ts`.

use pi_ai::api::simple_options::build_base_options;
use pi_ai::types::{
	AssistantContent, AssistantMessage, Context, Message, Model, ModelCost, StopReason, TextContent, Usage, UsageCost,
	UserMessage, UserMessageContent,
};
use pi_ai::utils::estimate::{ContextUsageEstimate, estimate_context_tokens};
use pi_ai::utils::transcript::normalize_context;

fn create_usage(total_tokens: f64) -> Usage {
	Usage {
		input: total_tokens,
		output: 0.0,
		cache_read: 0.0,
		cache_write: 0.0,
		total_tokens,
		cost: UsageCost::default(),
		..Usage::default()
	}
}

fn create_assistant(timestamp: f64, total_tokens: f64) -> Message {
	Message::Assistant(AssistantMessage {
		content: vec![AssistantContent::Text(TextContent {
			text: "kept".into(),
			..TextContent::default()
		})],
		api: "openai-responses".into(),
		provider: "openai".into(),
		model: "test-model".into(),
		usage: create_usage(total_tokens),
		stop_reason: StopReason::Stop,
		timestamp,
		..AssistantMessage::default()
	})
}

fn user(content: &str, timestamp: f64) -> Message {
	Message::User(UserMessage {
		content: UserMessageContent::Text(content.into()),
		timestamp,
		..UserMessage::default()
	})
}

fn model() -> Model {
	let mut model = Model {
		reasoning: false,
		context_window: 10_000.0,
		max_tokens: 8_000.0,
		..Model::default()
	};
	model.id = "test-model".into();
	model.name = "Test Model".into();
	model.api = "openai-responses".into();
	model.provider = "openai".into();
	model.base_url = "https://api.openai.com/v1".into();
	model.input = vec![pi_ai::types::ModelInputModality::Text];
	model.cost = ModelCost::default();
	model
}

mod context_token_estimation {
	use super::*;

	// Regression for #10497: large new inputs need more room than chars/4 allows.
	#[test]
	fn reserves_3_5_characters_per_token_for_new_text_when_limiting_output() {
		let context = normalize_context(&Context {
			messages: vec![create_assistant(100.0, 2_000.0), user(&"x".repeat(3_500), 200.0)],
			..Context::default()
		});

		assert_eq!(
			estimate_context_tokens(&context.messages),
			ContextUsageEstimate {
				tokens: 3_000.0,
				usage_tokens: 2_000.0,
				trailing_tokens: 1_000.0,
				last_usage_index: Some(0),
			}
		);
		assert_eq!(
			build_base_options(&model(), &context, None, None).max_tokens,
			Some(2_904.0)
		);
	}

	#[test]
	fn ignores_stale_assistant_usage_after_a_newer_message_is_inserted_before_it() {
		let context = normalize_context(&Context {
			system_prompt: Some("system".into()),
			messages: vec![
				user("summary", 200.0),
				create_assistant(100.0, 9_500.0),
				user(&"x".repeat(4_000), 300.0),
			],
			..Context::default()
		});

		assert_eq!(
			estimate_context_tokens(&context.messages),
			ContextUsageEstimate {
				tokens: 1_149.0,
				usage_tokens: 0.0,
				trailing_tokens: 1_149.0,
				last_usage_index: None,
			}
		);
		assert_eq!(
			build_base_options(&model(), &context, None, None).max_tokens,
			Some(4_755.0)
		);
	}

	#[test]
	fn uses_assistant_usage_again_after_a_response_to_the_inserted_context() {
		let context = normalize_context(&Context {
			messages: vec![
				user("summary", 200.0),
				create_assistant(100.0, 9_500.0),
				user("new prompt", 300.0),
				create_assistant(400.0, 2_000.0),
				user("tail", 500.0),
			],
			..Context::default()
		});

		assert_eq!(
			estimate_context_tokens(&context.messages),
			ContextUsageEstimate {
				tokens: 2_002.0,
				usage_tokens: 2_000.0,
				trailing_tokens: 2.0,
				last_usage_index: Some(3),
			}
		);
	}
}
