//! Port of `packages/ai/test/openai-responses-namespace.test.ts`.

mod openai_responses_support;

use indexmap::IndexMap;
use openai_responses_support::{assert_match_object, create_output, events, model, with};
use pi_ai::api::openai_responses_shared::{
	ConvertResponsesMessagesOptions, OpenAIResponsesStreamOptions, convert_responses_messages, process_responses_stream,
};
use pi_ai::types::{AssistantContent, AssistantMessage, Context, Message, Model, SharedAssistantMessage, ToolCall};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::run;
use serde_json::{Value, json};

fn gpt54() -> Model {
	model(json!({
		"id": "gpt-5.4",
		"name": "GPT-5.4",
		"api": "openai-responses",
		"provider": "openai",
		"baseUrl": "https://api.openai.com/v1",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 400000,
		"maxTokens": 128000,
	}))
}

fn create_function_call_events() -> Vec<Value> {
	vec![
		json!({
			"type": "response.output_item.added",
			"sequence_number": 0,
			"output_index": 0,
			"item": { "type": "function_call", "id": "fc_test", "call_id": "call_test", "name": "lookup", "arguments": "" },
		}),
		json!({
			"type": "response.output_item.done",
			"sequence_number": 1,
			"output_index": 0,
			"item": {
				"type": "function_call",
				"id": "fc_test",
				"call_id": "call_test",
				"name": "lookup",
				"arguments": "{\"value\":\"hello\"}",
				"namespace": "dynamic_tools",
			},
		}),
		json!({ "type": "response.completed", "sequence_number": 2, "response": { "id": "resp_test", "status": "completed" } }),
	]
}

fn create_custom_tool_call_events() -> Vec<Value> {
	vec![
		json!({
			"type": "response.output_item.added",
			"sequence_number": 0,
			"output_index": 0,
			"item": { "type": "custom_tool_call", "id": "ctc_test", "call_id": "call_test", "name": "query", "input": "" },
		}),
		json!({
			"type": "response.output_item.done",
			"sequence_number": 1,
			"output_index": 0,
			"item": {
				"type": "custom_tool_call",
				"id": "ctc_test",
				"call_id": "call_test",
				"name": "query",
				"input": "hello",
				"namespace": "dynamic_tools",
			},
		}),
		json!({ "type": "response.completed", "sequence_number": 2, "response": { "id": "resp_test", "status": "completed" } }),
	]
}

fn get_tool_call(output: &SharedAssistantMessage) -> Value {
	let output = output.borrow();
	let Some(AssistantContent::ToolCall(block)) = output.content.first() else {
		panic!("Expected toolCall block");
	};
	serde_json::to_value(block).expect("tool call serializes")
}

fn replay(target: &Model, output: &AssistantMessage, options: Option<&ConvertResponsesMessagesOptions>) -> Vec<Value> {
	let context = normalize_context(&Context {
		messages: vec![Message::Assistant(output.clone())],
		..Context::default()
	});
	convert_responses_messages(target, &context, &["openai"], options).unwrap()
}

fn grammar_tool_input_properties() -> IndexMap<String, String> {
	IndexMap::from([("query".to_string(), "input".to_string())])
}

fn tool_call(id: &str, name: &str, arguments: Value, namespace: Option<&str>) -> AssistantContent {
	AssistantContent::ToolCall(ToolCall {
		id: id.into(),
		name: name.into(),
		arguments: serde_json::from_value(arguments).expect("arguments are an object"),
		namespace: namespace.map(str::to_string),
		..ToolCall::default()
	})
}

mod open_ai_responses_terminal_messages {
	use super::*;

	#[test]
	fn omits_an_absent_error_message() {
		run(|_| async {
			let model = gpt54();
			let output = create_output(&model);
			process_responses_stream(
				events(create_function_call_events()),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				None,
			)
			.await
			.unwrap();

			assert!(
				serde_json::to_value(&*output.borrow())
					.unwrap()
					.get("errorMessage")
					.is_none()
			);
		});
	}
}

mod open_ai_responses_tool_call_namespaces {
	use super::*;

	#[test]
	fn round_trips_a_function_namespace_received_only_on_output_item_done() {
		run(|_| async {
			let model = gpt54();
			let output = create_output(&model);
			process_responses_stream(
				events(create_function_call_events()),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				None,
			)
			.await
			.unwrap();

			assert_match_object(
				&get_tool_call(&output),
				&json!({
					"id": "call_test|fc_test",
					"name": "lookup",
					"arguments": { "value": "hello" },
					"namespace": "dynamic_tools",
				}),
			);

			let replayed = replay(&model, &output.borrow(), None);
			let replayed = replayed
				.iter()
				.find(|item| item["type"] == "function_call")
				.expect("function_call");
			assert_match_object(
				replayed,
				&json!({
					"type": "function_call",
					"id": "fc_test",
					"call_id": "call_test",
					"name": "lookup",
					"arguments": "{\"value\":\"hello\"}",
					"namespace": "dynamic_tools",
				}),
			);
		});
	}

	#[test]
	fn round_trips_a_custom_tool_namespace_received_only_on_output_item_done() {
		run(|_| async {
			let model = gpt54();
			let output = create_output(&model);
			let stream_options = OpenAIResponsesStreamOptions {
				grammar_tool_input_properties: Some(grammar_tool_input_properties()),
				..OpenAIResponsesStreamOptions::default()
			};
			process_responses_stream(
				events(create_custom_tool_call_events()),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				Some(&stream_options),
			)
			.await
			.unwrap();

			assert_match_object(
				&get_tool_call(&output),
				&json!({
					"id": "call_test|ctc_test",
					"name": "query",
					"arguments": { "input": "hello" },
					"namespace": "dynamic_tools",
				}),
			);

			let convert_options = ConvertResponsesMessagesOptions {
				grammar_tool_input_properties: Some(grammar_tool_input_properties()),
				..ConvertResponsesMessagesOptions::default()
			};
			let replayed = replay(&model, &output.borrow(), Some(&convert_options));
			let replayed = replayed
				.iter()
				.find(|item| item["type"] == "custom_tool_call")
				.expect("custom_tool_call");
			assert_match_object(
				replayed,
				&json!({
					"type": "custom_tool_call",
					"id": "ctc_test",
					"call_id": "call_test",
					"name": "query",
					"input": "hello",
					"namespace": "dynamic_tools",
				}),
			);
		});
	}

	#[test]
	fn drops_namespaces_when_the_target_cannot_replay_their_load_items() {
		run(|_| async {
			let model = gpt54();
			let output = create_output(&model);
			output.borrow_mut().content.extend([
				tool_call(
					"call_function|fc_test",
					"lookup",
					json!({ "value": "hello" }),
					Some("dynamic_tools"),
				),
				tool_call(
					"call_custom|ctc_test",
					"query",
					json!({ "input": "hello" }),
					Some("dynamic_tools"),
				),
			]);
			let target_models = [
				with(&model, json!({ "id": "gpt-5.2", "name": "GPT-5.2" })),
				with(&model, json!({ "provider": "azure" })),
				with(
					&model,
					json!({
						"api": "openai-codex-responses",
						"provider": "openai-codex",
						"id": "gpt-5.3-codex-spark",
						"name": "GPT-5.3 Codex Spark",
					}),
				),
			];

			for target_model in &target_models {
				let convert_options = ConvertResponsesMessagesOptions {
					grammar_tool_input_properties: Some(grammar_tool_input_properties()),
					..ConvertResponsesMessagesOptions::default()
				};
				let replayed = replay(target_model, &output.borrow(), Some(&convert_options));
				let function_call = replayed.iter().find(|item| item["type"] == "function_call");
				let custom_tool_call = replayed.iter().find(|item| item["type"] == "custom_tool_call");
				let function_call = function_call.expect("function_call is defined");
				assert!(function_call.get("namespace").is_none(), "{}", target_model.id);
				let custom_tool_call = custom_tool_call.expect("custom_tool_call is defined");
				assert!(custom_tool_call.get("namespace").is_none(), "{}", target_model.id);
			}
		});
	}

	#[test]
	fn does_not_add_a_namespace_to_ordinary_function_calls() {
		run(|_| async {
			let model = gpt54();
			let output = create_output(&model);
			output.borrow_mut().content.push(tool_call(
				"call_test|fc_test",
				"lookup",
				json!({ "value": "hello" }),
				None,
			));

			let replayed = replay(&model, &output.borrow(), None);
			let replayed = replayed
				.iter()
				.find(|item| item["type"] == "function_call")
				.expect("function_call is defined");
			assert!(replayed.get("namespace").is_none());
		});
	}
}
