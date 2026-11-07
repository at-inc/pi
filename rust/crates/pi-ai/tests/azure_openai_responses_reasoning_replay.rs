//! Port of `packages/ai/test/azure-openai-responses-reasoning-replay.test.ts`.

mod openai_responses_support;

use openai_responses_support::{assert_match_object, context, create_output, events, model};
use pi_ai::api::openai_responses_shared::{convert_responses_messages, process_responses_stream};
use pi_ai::types::{Model, SharedAssistantMessage};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::testing::run;
use serde_json::{Value, json};

fn create_model() -> Model {
	model(json!({
		"id": "gpt-5-mini",
		"name": "GPT-5 Mini",
		"api": "azure-openai-responses",
		"provider": "azure",
		"baseUrl": "https://example.invalid",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 400000,
		"maxTokens": 128000,
	}))
}

fn create_events(done_item: &Value, completed_item: &Value) -> Vec<Value> {
	vec![
		json!({
			"type": "response.output_item.added",
			"output_index": 0,
			"sequence_number": 0,
			"item": { "type": "reasoning", "id": done_item["id"], "summary": [] },
		}),
		json!({ "type": "response.output_item.done", "output_index": 0, "sequence_number": 1, "item": done_item }),
		json!({
			"type": "response.completed",
			"sequence_number": 2,
			"response": { "id": "resp_test", "status": "completed", "output": [completed_item] },
		}),
	]
}

fn get_replayed_reasoning(model: &Model, assistant: &SharedAssistantMessage) -> Value {
	let now = pi_host::now_ms();
	let context = context(json!({
		"messages": [
			{ "role": "user", "content": "first", "timestamp": now - 1.0 },
			serde_json::to_value(&*assistant.borrow()).unwrap(),
			{ "role": "user", "content": "follow-up", "timestamp": now },
		],
	}));
	let input = convert_responses_messages(model, &context, &["azure"], None).unwrap();
	input
		.into_iter()
		.find(|item| item["type"] == "reasoning")
		.expect("reasoning item")
}

async fn replay(done_item: Value, completed_item: Value) -> Value {
	let model = create_model();
	let output = create_output(&model);
	process_responses_stream(
		events(create_events(&done_item, &completed_item)),
		&output,
		&AssistantMessageEventStream::new(),
		&model,
		None,
	)
	.await
	.unwrap();
	get_replayed_reasoning(&model, &output)
}

mod azure_open_ai_responses_reasoning_replay {
	use super::*;

	#[test]
	fn preserves_existing_encrypted_content_from_output_item_done() {
		run(|_| async {
			let done_item = json!({ "type": "reasoning", "id": "rs_done", "summary": [], "encrypted_content": "from-output-item-done" });
			let mut completed_item = done_item.clone();
			completed_item["encrypted_content"] = json!("from-response-completed");

			assert_match_object(
				&replay(done_item, completed_item).await,
				&json!({ "type": "reasoning", "id": "rs_done", "encrypted_content": "from-output-item-done" }),
			);
		});
	}

	#[test]
	fn fills_encrypted_content_when_output_item_done_omitted_it() {
		run(|_| async {
			let done_item = json!({ "type": "reasoning", "id": "rs_missing", "summary": [] });
			let mut completed_item = done_item.clone();
			completed_item["encrypted_content"] = json!("from-response-completed");

			assert_match_object(
				&replay(done_item, completed_item).await,
				&json!({ "type": "reasoning", "id": "rs_missing", "encrypted_content": "from-response-completed" }),
			);
		});
	}
}
