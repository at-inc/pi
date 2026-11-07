//! Port of `packages/ai/test/google-shared-image-tool-result-routing.test.ts`.
//!
//! Models and contexts are built from the TS object literals as JSON. `Date.now()` is a fixed time.

use pi_ai::api::google_shared::convert_messages;
use pi_ai::types::{Context, Model};
use pi_ai::utils::transcript::normalize_context;
use serde_json::{Value, json};

const NOW: f64 = 1_700_000_000_000.0;

fn make_model(id: &str) -> Model {
	serde_json::from_value(json!({
		"id": id,
		"name": id,
		"api": "google-generative-ai",
		"provider": "google",
		"baseUrl": "https://example.com",
		"reasoning": true,
		"input": ["text", "image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 8192,
	}))
	.unwrap()
}

fn make_context_json(id: &str) -> Value {
	json!({
		"messages": [
			{ "role": "user", "content": "read the files", "timestamp": NOW },
			{
				"role": "assistant",
				"content": [
					{ "type": "toolCall", "id": "call_a", "name": "read", "arguments": { "path": "a.txt" } },
					{ "type": "toolCall", "id": "call_img", "name": "read", "arguments": { "path": "image.png" } },
					{ "type": "toolCall", "id": "call_b", "name": "read", "arguments": { "path": "b.txt" } },
				],
				"api": "google-generative-ai",
				"provider": "google",
				"model": id,
				"usage": {
					"input": 0,
					"output": 0,
					"cacheRead": 0,
					"cacheWrite": 0,
					"totalTokens": 0,
					"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
				},
				"stopReason": "toolUse",
				"timestamp": NOW,
			},
			{
				"role": "toolResult",
				"toolCallId": "call_a",
				"toolName": "read",
				"content": [{ "type": "text", "text": "alpha text" }],
				"isError": false,
				"timestamp": NOW,
			},
			{
				"role": "toolResult",
				"toolCallId": "call_img",
				"toolName": "read",
				"content": [{ "type": "image", "data": "abc", "mimeType": "image/png" }],
				"isError": false,
				"timestamp": NOW,
			},
			{
				"role": "toolResult",
				"toolCallId": "call_b",
				"toolName": "read",
				"content": [{ "type": "text", "text": "beta text" }],
				"isError": false,
				"timestamp": NOW,
			},
		],
	})
}

fn convert(model: &Model, context: Value) -> Vec<Value> {
	let context: Context = serde_json::from_value(context).unwrap();
	pi_host::testing::run(|_| async { convert_messages(model, &normalize_context(&context)) })
}

mod google_shared_image_tool_result_routing {
	use super::*;

	#[test]
	fn keeps_separate_synthetic_image_turn_for_gemini_2_x_google_api_models() {
		let model = make_model("gemini-2.5-flash");
		let contents = convert(&model, make_context_json(&model.id));

		assert_eq!(contents.len(), 5);
		assert!(
			contents[2]["parts"]
				.as_array()
				.unwrap()
				.iter()
				.all(|part| part.get("functionResponse").is_some())
		);
		assert_eq!(contents[3]["parts"][0]["text"], "Tool result image:");
		assert!(contents[3]["parts"][1].get("inlineData").is_some());
		assert!(contents[4]["parts"][0].get("functionResponse").is_some());
	}

	#[test]
	fn nests_image_tool_results_for_gemini_3_google_api_models() {
		let model = make_model("gemini-3-pro-preview");
		let contents = convert(&model, make_context_json(&model.id));

		assert_eq!(contents.len(), 3);
		let tool_result_turn = &contents[2];
		assert_eq!(tool_result_turn["parts"].as_array().unwrap().len(), 3);
		let image_response = &tool_result_turn["parts"][1]["functionResponse"];
		assert!(image_response.is_object());
		assert_eq!(image_response["parts"].as_array().unwrap().len(), 1);
		assert!(image_response["parts"][0].get("inlineData").is_some());
	}

	// https://github.com/earendil-works/pi/issues/8720
	#[test]
	fn treats_whitespace_only_tool_result_text_as_no_text() {
		let model = make_model("gemini-3-pro-preview");
		let mut context = make_context_json(&model.id);
		for message in context["messages"].as_array_mut().unwrap() {
			if message["role"] != "toolResult" {
				continue;
			}
			if message["toolCallId"] == "call_a" {
				message["content"] = json!([{ "type": "text", "text": "\r\n" }]);
			}
			if message["toolCallId"] == "call_img" {
				message["content"]
					.as_array_mut()
					.unwrap()
					.insert(0, json!({ "type": "text", "text": " \n" }));
			}
		}
		let contents = convert(&model, context);

		let responses: Vec<&Value> = contents[2]["parts"]
			.as_array()
			.unwrap()
			.iter()
			.map(|part| &part["functionResponse"]["response"])
			.collect();
		assert_eq!(responses[0], &json!({ "output": "" }));
		assert_eq!(responses[1], &json!({ "output": "(see attached image)" }));
		assert_eq!(responses[2], &json!({ "output": "beta text" }));
	}
}
