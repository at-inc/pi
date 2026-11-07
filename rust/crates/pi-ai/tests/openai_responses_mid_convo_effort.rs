//! Port of `packages/ai/test/openai-responses-mid-convo-effort.test.ts`.

mod openai_responses_support;

use std::cell::RefCell;
use std::rc::Rc;

use openai_responses_support::{context, model, response};
use pi_ai::api::openai_responses::{OpenAIResponsesOptions, stream};
use pi_ai::types::{AssistantMessage, Model, StopReason, ThinkingLevel};
use pi_host::FetchFunction;
use pi_host::testing::run;
use serde_json::{Value, json};

fn open_ai_model(compat: Value) -> Model {
	model(json!({
		"id": "gpt-6-luna",
		"name": "GPT-6 Luna",
		"api": "openai-responses",
		"provider": "openai",
		"baseUrl": "https://api.openai.com/v1",
		"reasoning": true,
		"thinkingLevelMap": {
			"off": "none",
			"minimal": null,
			"low": "low",
			"medium": "medium",
			"high": "high",
			"xhigh": "xhigh",
			"max": "max",
		},
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 272000,
		"maxTokens": 128000,
		"compat": compat,
	}))
}

fn default_model() -> Model {
	open_ai_model(json!({ "supportsMidConvoEffort": true }))
}

fn user(text: &str) -> Value {
	json!({ "role": "user", "content": text, "timestamp": 0 })
}

fn tool_result(call_id: &str) -> Value {
	json!({
		"role": "toolResult",
		"toolCallId": call_id,
		"toolName": "lookup",
		"content": [{ "type": "text", "text": "result" }],
		"isError": false,
		"timestamp": 0,
	})
}

fn update(effort: &str) -> Value {
	json!({ "type": "configuration_update", "reasoning": { "effort": effort } })
}

fn response_events(response_id: &str, with_tool_call: bool) -> Vec<Value> {
	let item = if with_tool_call {
		json!({ "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "lookup", "arguments": "{}", "status": "completed" })
	} else {
		json!({
			"type": "message",
			"id": format!("msg_{response_id}"),
			"role": "assistant",
			"status": "completed",
			"content": [{ "type": "output_text", "text": "answer", "annotations": [] }],
		})
	};
	vec![
		json!({ "type": "response.created", "response": { "id": response_id } }),
		json!({ "type": "response.output_item.added", "output_index": 0, "item": item }),
		json!({ "type": "response.output_item.done", "output_index": 0, "item": item }),
		json!({ "type": "response.completed", "response": { "id": response_id, "status": "completed" } }),
	]
}

struct Run {
	payload: Value,
	message: AssistantMessage,
}

#[derive(Default)]
struct RunOptions {
	api_key: Option<&'static str>,
	model: Option<Model>,
	with_tool_call: bool,
}

async fn run_open_ai(messages: &[Value], reasoning_effort: Option<ThinkingLevel>, options: RunOptions) -> Run {
	let payload = Rc::new(RefCell::new(None));
	let captured = payload.clone();
	let with_tool_call = options.with_tool_call;
	let fetch_mock: FetchFunction = Rc::new(move |request| {
		*captured.borrow_mut() = Some(
			serde_json::from_slice::<Value>(request.body.as_deref().expect("request has a body"))
				.expect("request body is JSON"),
		);
		let body: String = response_events("resp_1", with_tool_call)
			.iter()
			.map(|event| format!("data: {}\n\n", pi_host::js::json_stringify(event)))
			.collect();
		Box::pin(async move { Ok(response(200, &[("content-type", "text/event-stream")], body)) })
	});
	let mut stream_options = OpenAIResponsesOptions {
		reasoning_effort,
		..OpenAIResponsesOptions::default()
	};
	stream_options.api_key = Some(options.api_key.unwrap_or("sk-test").into());
	stream_options.fetch = Some(fetch_mock);
	let model = options.model.unwrap_or_else(default_model);
	let message = stream(&model, &context(json!({ "messages": messages })), Some(stream_options))
		.result()
		.await
		.expect("stream has a result");
	let payload = payload.borrow_mut().take().expect("Request payload was not captured");
	assert_ne!(message.stop_reason, StopReason::Error, "{:?}", message.error_message);
	Run { payload, message }
}

fn message_json(message: &AssistantMessage) -> Value {
	serde_json::to_value(message).expect("message serializes")
}

fn input(run: &Run) -> &Vec<Value> {
	run.payload["input"].as_array().expect("input is an array")
}

fn configuration_updates(items: &[Value]) -> Vec<Value> {
	items
		.iter()
		.filter(|item| item["type"] == "configuration_update")
		.cloned()
		.collect()
}

mod open_ai_responses_configuration_update {
	use super::*;

	#[test]
	fn keeps_reasoning_effort_constant_and_only_appends_input_when_the_level_changes() {
		run(|_| async {
			let mut history = vec![user("one")];
			let first = run_open_ai(&history, Some(ThinkingLevel::High), RunOptions::default()).await;
			history.extend([message_json(&first.message), user("two")]);
			let second = run_open_ai(&history, Some(ThinkingLevel::High), RunOptions::default()).await;
			history.extend([message_json(&second.message), user("three")]);
			let third = run_open_ai(&history, Some(ThinkingLevel::Low), RunOptions::default()).await;

			assert_eq!(
				*input(&first),
				vec![
					json!({ "role": "user", "content": [{ "type": "input_text", "text": "one" }] }),
					update("high")
				]
			);
			for run in [&first, &second, &third] {
				assert_eq!(run.payload["reasoning"]["effort"], "medium");
			}
			assert_eq!(input(&second)[..input(&first).len()], input(&first)[..]);
			assert_eq!(input(&third)[..input(&second).len()], input(&second)[..]);
			assert_eq!(
				configuration_updates(input(&third)),
				[update("high"), update("high"), update("low")]
			);
			assert_eq!(input(&third).last(), Some(&update("low")));
			assert_eq!(
				[&first, &second, &third].map(|run| run.message.provider_thinking_level.clone()),
				[
					Some("high".to_string()),
					Some("high".to_string()),
					Some("low".to_string())
				]
			);
		});
	}

	#[test]
	fn applies_a_level_change_made_during_a_tool_loop_to_the_next_reply() {
		run(|_| async {
			let mut history = vec![user("one")];
			let tool_call = run_open_ai(
				&history,
				Some(ThinkingLevel::High),
				RunOptions {
					with_tool_call: true,
					..RunOptions::default()
				},
			)
			.await;
			history.extend([message_json(&tool_call.message), tool_result("call_1|fc_1")]);
			let after_tool = run_open_ai(&history, Some(ThinkingLevel::Low), RunOptions::default()).await;

			assert_eq!(input(&after_tool)[..input(&tool_call).len()], input(&tool_call)[..]);
			let last_two = &input(&after_tool)[input(&after_tool).len() - 2..];
			assert_eq!(last_two[0]["type"], "function_call_output");
			assert_eq!(last_two[0]["call_id"], "call_1");
			assert_eq!(last_two[1], update("low"));
			assert_eq!(after_tool.message.provider_thinking_level.as_deref(), Some("low"));
		});
	}

	#[test]
	fn adds_items_only_before_replies_recorded_by_this_model() {
		run(|_| async {
			let unstamped = run_open_ai(
				&[user("one")],
				Some(ThinkingLevel::High),
				RunOptions {
					model: Some(open_ai_model(json!({}))),
					..RunOptions::default()
				},
			)
			.await
			.message;
			let stamped = |patch: Value| {
				let mut message = message_json(&unstamped);
				for (key, value) in patch.as_object().expect("patch is an object") {
					message[key] = value.clone();
				}
				message
			};
			let other_provider = stamped(json!({ "provider": "opencode", "providerThinkingLevel": "high" }));
			let other_model = stamped(json!({ "model": "gpt-6-sol", "providerThinkingLevel": "high" }));
			let recorded = stamped(json!({ "providerThinkingLevel": "high" }));

			let Run { payload, .. } = run_open_ai(
				&[
					user("one"),
					message_json(&unstamped),
					user("two"),
					other_provider,
					user("three"),
					other_model,
					user("four"),
					recorded,
					user("five"),
				],
				Some(ThinkingLevel::Low),
				RunOptions::default(),
			)
			.await;

			let items = payload["input"].as_array().expect("input is an array");
			let kinds: Vec<&Value> = items
				.iter()
				.map(|item| item.get("type").unwrap_or(&item["role"]))
				.collect();
			assert_eq!(
				kinds,
				[
					"user",
					"message",
					"user",
					"message",
					"user",
					"message",
					"user",
					"configuration_update",
					"message",
					"user",
					"configuration_update",
				]
			);
			assert_eq!(configuration_updates(items), [update("high"), update("low")]);
		});
	}

	#[test]
	fn sends_the_level_as_reasoning_effort_with_name() {
		run(|_| async {
			let cases = [
				(
					"Sign in with ChatGPT",
					"chatgpt-access-token",
					default_model(),
					Some(ThinkingLevel::Low),
					"low",
				),
				(
					"the compat flag turned off",
					"sk-test",
					open_ai_model(json!({ "supportsMidConvoEffort": false })),
					Some(ThinkingLevel::Low),
					"low",
				),
				("thinking off", "sk-test", default_model(), None, "none"),
			];
			for (name, api_key, model, level, effort) in cases {
				let first = run_open_ai(
					&[user("one")],
					Some(ThinkingLevel::High),
					RunOptions {
						api_key: Some(api_key),
						model: Some(model.clone()),
						..RunOptions::default()
					},
				)
				.await;
				let Run { payload, message } = run_open_ai(
					&[user("one"), message_json(&first.message), user("two")],
					level,
					RunOptions {
						api_key: Some(api_key),
						model: Some(model),
						..RunOptions::default()
					},
				)
				.await;

				assert_eq!(payload["reasoning"]["effort"], effort, "{name}");
				assert!(
					!payload["input"]
						.as_array()
						.unwrap()
						.iter()
						.any(|item| item["type"] == "configuration_update"),
					"{name}"
				);
				assert_eq!(message.provider_thinking_level, None, "{name}");
			}
		});
	}
}
