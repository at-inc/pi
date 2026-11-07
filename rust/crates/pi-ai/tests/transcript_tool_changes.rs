//! Port of `packages/ai/test/transcript-tool-changes.test.ts`.
//!
//! The single `transcript system messages` describe is split by transport: `transcript_system_messages` holds
//! "resolves the top-level tool boundary per transport" (`resolveTranscriptTools` alone), and the
//! `transcript_system_messages_*` modules hold the payload cases of the Anthropic Messages, OpenAI Responses and OpenAI
//! Completions APIs in TS order. `toMatchObject` on a list with `expect.stringContaining` entries checks those entries
//! with `contains` and the rest with `assert_match_object`.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::stream_simple;
use pi_ai::types::{Context, Message, Model, ProviderRequestOptions, SimpleStreamOptions, StreamOptions, Tool};
use pi_ai::utils::transcript::{TranscriptToolSupport, TranscriptTools, resolve_transcript_tools};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

fn tool(name: &str) -> Tool {
	Tool {
		name: name.into(),
		description: format!("{name} tool"),
		parameters: json!({ "type": "object", "properties": {} }),
		..Tool::default()
	}
}

fn described(tool: Tool, description: &str) -> Tool {
	Tool {
		description: description.into(),
		..tool
	}
}

fn tool_json(tool: &Tool) -> Value {
	serde_json::to_value(tool).unwrap()
}

mod transcript_system_messages {
	use super::*;

	#[test]
	fn resolves_the_top_level_tool_boundary_per_transport() {
		let base_tool = tool("base_tool");
		let drop_tool = tool("drop_tool");
		let late_tool = tool("late_tool");
		let messages: Vec<Message> = serde_json::from_value(json!([
			{ "role": "system", "content": "base prompt", "toolsAdded": [tool_json(&base_tool), tool_json(&drop_tool)], "timestamp": 0 },
			{ "role": "system", "content": "", "toolsRemoved": [{ "name": "drop_tool" }], "timestamp": 1 },
			{ "role": "system", "content": "", "toolsAdded": [tool_json(&drop_tool)], "timestamp": 2 },
			{ "role": "system", "content": "", "toolsAdded": [tool_json(&described(late_tool.clone(), "x"))], "timestamp": 3 },
			{ "role": "system", "content": "", "toolsAdded": [tool_json(&described(late_tool.clone(), "y"))], "timestamp": 4 },
		]))
		.unwrap();

		assert_eq!(
			resolve_transcript_tools(&messages[..2], TranscriptToolSupport::Additions),
			TranscriptTools {
				request_tools: vec![base_tool.clone()],
				folded_system_messages: 2,
			}
		);
		// Kimi K3 ignores a removed tool that a later tool message declares again, so the re-added
		// drop_tool moves the boundary; the new late_tool after it stays anchored.
		assert_eq!(
			resolve_transcript_tools(&messages[..4], TranscriptToolSupport::Additions),
			TranscriptTools {
				request_tools: vec![base_tool.clone(), drop_tool.clone()],
				folded_system_messages: 3,
			}
		);
		// The redefinition of late_tool moves the boundary to its message.
		let all_tools = vec![base_tool, drop_tool, described(late_tool, "y")];
		assert_eq!(
			resolve_transcript_tools(&messages, TranscriptToolSupport::Additions),
			TranscriptTools {
				request_tools: all_tools.clone(),
				folded_system_messages: 5,
			}
		);
		assert_eq!(
			resolve_transcript_tools(&messages, TranscriptToolSupport::Changes),
			TranscriptTools {
				request_tools: vec![],
				folded_system_messages: 0,
			}
		);
		assert_eq!(
			resolve_transcript_tools(&messages, TranscriptToolSupport::None),
			TranscriptTools {
				request_tools: all_tools,
				folded_system_messages: 5,
			}
		);
	}
}

/// `expect(actual).toMatchObject(expected)` for JSON values.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len(), "{actual:?}");
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		_ => assert_eq!(actual, expected),
	}
}

/// `capturePayload(model, context)`: `streamSimple` with an `onPayload` that throws.
fn capture_payload(model: &Model, context: Value) -> Value {
	let context: Context = serde_json::from_value(context).unwrap();
	run(|_| async move {
		let captured: Rc<RefCell<Option<Value>>> = Rc::default();
		let sink = captured.clone();
		let base = StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("test-key".into()),
				on_payload: Some(Rc::new(move |payload, _| {
					*sink.borrow_mut() = Some(payload);
					Box::pin(async { Err(PiError::new("payload captured")) })
				})),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		};
		let options = SimpleStreamOptions {
			base,
			..SimpleStreamOptions::default()
		};
		stream_simple(model, &context, Some(options)).unwrap().result().await;
		captured.borrow_mut().take().expect("Expected payload capture")
	})
}

fn anthropic_model(id: &str, name: &str, compat: Option<Value>) -> Model {
	let mut model = json!({
		"baseUrl": "http://127.0.0.1:9",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 100000,
		"maxTokens": 1000,
		"id": id,
		"name": name,
		"api": "anthropic-messages",
		"provider": "anthropic",
	});
	if let Some(compat) = compat {
		model["compat"] = compat;
	}
	serde_json::from_value(model).unwrap()
}

fn anthropic_native_model() -> Model {
	anthropic_model(
		"claude-opus-5",
		"Claude Opus 5",
		Some(json!({ "supportsMidConvoSystemMessages": true, "supportsMidConvoToolChanges": true })),
	)
}

fn base_tool() -> Value {
	tool_json(&tool("base_tool"))
}

fn late_tool() -> Value {
	tool_json(&tool("late_tool"))
}

fn context_messages() -> Vec<Value> {
	vec![
		json!({
			"role": "system",
			"content": "base prompt",
			"sections": { "rules": "<rules>\nold rules\n</rules>", "docs": "<docs>\nread docs\n</docs>" },
			"toolsAdded": [base_tool()],
			"timestamp": 0,
		}),
		json!({ "role": "user", "content": "before", "timestamp": 1 }),
		json!({
			"role": "system",
			"content": "updated guidance",
			"sections": { "rules": "<rules>\nnew rules\n</rules>", "docs": null },
			"toolsRemoved": [{ "name": "base_tool" }],
			"toolsAdded": [late_tool()],
			"timestamp": 2,
		}),
	]
}

fn tools_context() -> Value {
	json!({ "messages": context_messages() })
}

fn betas(payload: &Value) -> Vec<String> {
	payload
		.get("betas")
		.and_then(Value::as_array)
		.map(|betas| betas.iter().map(|beta| beta.as_str().unwrap().to_string()).collect())
		.unwrap_or_default()
}

fn names(values: &Value) -> Vec<String> {
	values
		.as_array()
		.unwrap()
		.iter()
		.map(|value| value["name"].as_str().unwrap().to_string())
		.collect()
}

fn roles(payload: &Value) -> Vec<String> {
	payload["messages"]
		.as_array()
		.unwrap()
		.iter()
		.map(|message| message["role"].as_str().unwrap().to_string())
		.collect()
}

fn last_message(payload: &Value) -> &Value {
	payload["messages"].as_array().unwrap().last().unwrap()
}

fn reply(text: &str, provider: &str, api: &str, model: &str) -> Value {
	json!({
		"role": "assistant",
		"content": [{ "type": "text", "text": text }],
		"api": api,
		"provider": provider,
		"model": model,
		"usage": {
			"input": 0,
			"output": 0,
			"cacheRead": 0,
			"cacheWrite": 0,
			"totalTokens": 0,
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
		},
		"stopReason": "stop",
		"timestamp": 3,
	})
}

mod transcript_system_messages_anthropic {
	use super::*;

	#[test]
	fn sends_anthropic_updates_and_tool_changes_in_native_system_messages() {
		let payload = capture_payload(&anthropic_native_model(), tools_context());

		assert!(betas(&payload).contains(&"inline-tools-2026-09-15".to_string()));
		assert!(!betas(&payload).contains(&"mid-conversation-tool-changes-2026-07-01".to_string()));
		let system: Vec<&str> = payload["system"]
			.as_array()
			.unwrap()
			.iter()
			.map(|block| block["text"].as_str().unwrap())
			.collect();
		assert_eq!(
			system,
			["base prompt\n\n<rules>\nold rules\n</rules>\n\n<docs>\nread docs\n</docs>"]
		);
		// The top-level list holds only the initial tools (with the cache breakpoint) and the
		// deferred placeholder; later tools never appear there.
		assert_match_object(
			&payload["tools"],
			&json!([
				{ "name": "base_tool", "cache_control": { "type": "ephemeral" } },
				{ "name": "__pi_deferred_placeholder__", "defer_loading": true },
			]),
		);
		assert_eq!(payload["tools"][0].get("defer_loading"), None);
		assert_eq!(payload["tools"][1].get("cache_control"), None);
		let update = last_message(&payload);
		assert_match_object(
			update,
			&json!({
				"role": "system",
				"content": [
					{ "type": "text" },
					{ "type": "tool_removal", "tool": { "type": "tool_reference", "name": "base_tool" } },
					{
						"type": "tool_addition",
						"tool": { "type": "tool_definition", "definition": { "name": "late_tool", "description": "late_tool tool" } },
						"cache_control": { "type": "ephemeral" },
					},
				],
			}),
		);
		// Cache control goes on the block, never on the inline definition; nothing is deferred.
		let definition = &update["content"][2]["tool"]["definition"];
		assert_eq!(definition.get("cache_control"), None);
		assert_eq!(definition.get("defer_loading"), None);
		let text = update["content"][0]["text"].as_str().unwrap();
		assert!(text.contains("updated guidance"));
		assert!(text.contains("<rules>\nnew rules\n</rules>"));
		assert!(text.contains("Removed system prompt section \"docs\""));

		// The placeholder is declared before any change so its scaffolding is cached from request one.
		let initial = capture_payload(
			&anthropic_native_model(),
			json!({ "messages": context_messages()[..2].to_vec() }),
		);
		assert_eq!(names(&initial["tools"]), ["base_tool", "__pi_deferred_placeholder__"]);
	}

	#[test]
	fn redefines_an_anthropic_tool_by_value_under_the_same_name() {
		let redefined_tool = tool_json(&described(tool("base_tool"), "changed"));
		let payload = capture_payload(
			&anthropic_native_model(),
			json!({
				"messages": [
					{ "role": "system", "content": "base prompt", "toolsAdded": [base_tool()], "timestamp": 0 },
					{ "role": "user", "content": "before", "timestamp": 1 },
					{
						"role": "system",
						"content": "",
						"toolsRemoved": [{ "name": "base_tool" }],
						"toolsAdded": [redefined_tool],
						"timestamp": 2,
					},
				],
			}),
		);

		assert!(betas(&payload).contains(&"inline-tools-2026-09-15".to_string()));
		// The top-level declaration keeps the original definition, so the cached prefix survives.
		assert_match_object(
			&payload["tools"],
			&json!([
				{ "name": "base_tool", "description": "base_tool tool" },
				{ "name": "__pi_deferred_placeholder__" },
			]),
		);
		// The new definition replaces the old one in place; no removal block is sent for it.
		assert_match_object(
			last_message(&payload),
			&json!({
				"role": "system",
				"content": [
					{
						"type": "tool_addition",
						"tool": { "type": "tool_definition", "definition": { "name": "base_tool", "description": "changed" } },
					},
				],
			}),
		);
	}

	#[test]
	fn sends_the_current_anthropic_tool_list_without_an_initial_tool() {
		// Anthropic rejects a tool list where every tool (here: the placeholder) is deferred.
		let payload = capture_payload(
			&anthropic_native_model(),
			json!({
				"messages": [
					{ "role": "system", "content": "base prompt", "timestamp": 0 },
					{ "role": "user", "content": "before", "timestamp": 1 },
					{ "role": "system", "content": "updated guidance", "toolsAdded": [late_tool()], "timestamp": 2 },
				],
			}),
		);
		assert!(!betas(&payload).contains(&"inline-tools-2026-09-15".to_string()));
		assert_match_object(
			&payload["tools"],
			&json!([{ "name": "late_tool", "cache_control": { "type": "ephemeral" } }]),
		);
		assert_eq!(payload["tools"][0].get("defer_loading"), None);
		let types: Vec<&str> = last_message(&payload)["content"]
			.as_array()
			.unwrap()
			.iter()
			.map(|block| block["type"].as_str().unwrap())
			.collect();
		assert_eq!(types, ["text"]);
	}

	// #11091
	#[test]
	fn holds_an_anthropic_update_past_an_assistant_message_that_converts_to_no_blocks() {
		let messages = vec![
			json!({ "role": "system", "content": "base prompt", "timestamp": 0 }),
			json!({ "role": "user", "content": "hi", "timestamp": 1 }),
			json!({ "role": "system", "content": "updated guidance", "timestamp": 2 }),
			// Another model's empty reply: Anthropic rejects empty text blocks, so it is dropped.
			reply("", "openai-codex", "openai-codex-responses", "gpt-x"),
			json!({ "role": "user", "content": "again", "timestamp": 4 }),
		];
		let model = anthropic_native_model();

		// No later assistant: the update goes at the end.
		let at_end = capture_payload(&model, json!({ "messages": messages.clone() }));
		assert_eq!(roles(&at_end), ["user", "user", "system"]);

		// A later assistant that is sent: the update goes directly before it.
		let mut with_assistant = messages;
		with_assistant.push(reply("answer", &model.provider, &model.api, &model.id));
		with_assistant.push(json!({ "role": "user", "content": "third", "timestamp": 5 }));
		let before_assistant = capture_payload(&model, json!({ "messages": with_assistant }));
		assert_eq!(
			roles(&before_assistant),
			["user", "user", "system", "assistant", "user"]
		);
	}

	#[test]
	fn folds_anthropic_updates_into_the_system_prompt_without_native_support() {
		let model = anthropic_model("claude-sonnet-4-5", "Claude Sonnet 4.5", None);
		let payload = capture_payload(&model, tools_context());

		assert!(!betas(&payload).contains(&"inline-tools-2026-09-15".to_string()));
		let system: Vec<&str> = payload["system"]
			.as_array()
			.unwrap()
			.iter()
			.map(|block| block["text"].as_str().unwrap())
			.collect();
		assert_eq!(
			system,
			["base prompt\n\nupdated guidance\n\n<rules>\nnew rules\n</rules>"]
		);
		assert_eq!(names(&payload["tools"]), ["late_tool"]);
		assert_eq!(roles(&payload), ["user"]);
	}

	#[test]
	fn requires_both_anthropic_capabilities_for_native_tool_changes() {
		let model = anthropic_model(
			"claude-opus-5",
			"Claude Opus 5",
			Some(json!({ "supportsMidConvoToolChanges": true })),
		);
		let payload = capture_payload(&model, tools_context());

		assert!(!betas(&payload).contains(&"inline-tools-2026-09-15".to_string()));
		assert_eq!(names(&payload["tools"]), ["late_tool"]);
		assert_eq!(roles(&payload), ["user"]);
	}
}

/// `{ ...modelBase, ...fields }`.
fn base_model(fields: Value) -> Model {
	let mut model = json!({
		"baseUrl": "http://127.0.0.1:9",
		"reasoning": true,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 100000,
		"maxTokens": 1000,
	});
	for (key, value) in fields.as_object().unwrap() {
		model[key] = value.clone();
	}
	serde_json::from_value(model).unwrap()
}

fn additional_tools_model() -> Model {
	base_model(json!({
		"id": "gpt-5.4",
		"name": "GPT-5.4",
		"api": "openai-responses",
		"provider": "openai",
		"compat": { "supportsMidConvoSystemMessages": true, "supportsAdditionalTools": true },
	}))
}

fn addition_context() -> Value {
	json!({
		"messages": [
			{ "role": "system", "content": "base prompt", "toolsAdded": [base_tool()], "timestamp": 0 },
			{ "role": "user", "content": "before", "timestamp": 1 },
			{ "role": "system", "content": "updated guidance", "toolsAdded": [late_tool()], "timestamp": 2 },
		],
	})
}

fn drop_tool() -> Value {
	tool_json(&tool("drop_tool"))
}

/// Capture one request after each step: initial tools, removal of drop_tool, then two loads.
/// Returns the payloads after the removal and after each load, plus the one before the removal.
fn capture_tool_history(model: &Model) -> Vec<Value> {
	let steps = [
		vec![json!({ "role": "user", "content": "first", "timestamp": 1 })],
		vec![
			json!({ "role": "system", "content": "", "toolsRemoved": [{ "name": "drop_tool" }], "timestamp": 2 }),
			json!({ "role": "user", "content": "after removal", "timestamp": 3 }),
		],
		vec![
			json!({ "role": "system", "content": "", "toolsAdded": [late_tool()], "timestamp": 4 }),
			json!({ "role": "user", "content": "after first load", "timestamp": 5 }),
		],
		vec![
			json!({ "role": "system", "content": "", "toolsAdded": [tool_json(&tool("later_tool"))], "timestamp": 6 }),
			json!({ "role": "user", "content": "after second load", "timestamp": 7 }),
		],
	];
	let mut messages = vec![
		json!({ "role": "system", "content": "base prompt", "toolsAdded": [base_tool(), drop_tool()], "timestamp": 0 }),
	];
	let mut payloads = Vec::new();
	for step in steps {
		messages.extend(step);
		payloads.push(capture_payload(model, json!({ "messages": messages.clone() })));
	}
	payloads
}

/// Every request's message list starts with the previous request's message list.
fn expect_growing_prefix(inputs: &[&Value]) {
	for index in 1..inputs.len() {
		let previous = inputs[index - 1].as_array().unwrap();
		let current = inputs[index].as_array().unwrap();
		assert_eq!(&current[..previous.len()], previous.as_slice(), "request {index}");
	}
}

fn items(payload: &Value) -> &Vec<Value> {
	payload["input"].as_array().unwrap()
}

/// `input.filter((item) => item.type === kind)`.
fn items_of_type<'a>(payload: &'a Value, kind: &str) -> Vec<&'a Value> {
	items(payload).iter().filter(|item| item["type"] == kind).collect()
}

fn content(item: &Value) -> &str {
	item["content"].as_str().unwrap_or_default()
}

/// `tools?.map((value) => value.function?.name)`.
fn function_names(tools: &Value) -> Vec<String> {
	tools
		.as_array()
		.unwrap()
		.iter()
		.map(|value| value["function"]["name"].as_str().unwrap().to_string())
		.collect()
}

fn messages(payload: &Value) -> &Vec<Value> {
	payload["messages"].as_array().unwrap()
}

fn builtin(provider: &str, id: &str) -> Model {
	pi_ai::providers::all::get_builtin_model(provider, id).unwrap_or_else(|| panic!("{provider}/{id} is defined"))
}

mod transcript_system_messages_openai_responses {
	use super::*;

	#[test]
	fn declares_every_openai_tool_in_additional_tools_at_its_developer_message() {
		let payload = capture_payload(&additional_tools_model(), addition_context());

		// A later additional_tools item cannot replace a top-level tool, so no tool is declared there.
		assert!(payload.get("tools").is_none());
		let declared: Vec<&str> = items_of_type(&payload, "additional_tools")
			.iter()
			.map(|item| item["tools"][0]["name"].as_str().unwrap())
			.collect();
		assert_eq!(declared, ["base_tool", "late_tool"]);
		// The incremental notice keeps models from reading a later item as the complete tool set.
		let input = items(&payload);
		assert_eq!(input.len(), 6);
		assert_eq!(input[0]["type"], "additional_tools");
		assert_eq!(input[1]["content"], "base prompt");
		assert!(input[2]["content"].is_array());
		assert!(content(&input[3]).contains("incremental tools update"));
		assert_eq!(input[4]["type"], "additional_tools");
		assert_eq!(input[5]["content"], "updated guidance");
	}

	#[test]
	fn maps_system_message_additions_into_synthetic_tool_search() {
		let model = base_model(json!({
			"id": "gpt-5.4",
			"name": "GPT-5.4",
			"api": "openai-responses",
			"provider": "openai",
			"compat": { "supportsMidConvoSystemMessages": true, "supportsToolSearch": true },
		}));
		let payload = capture_payload(&model, addition_context());

		assert_eq!(names(&payload["tools"]), ["base_tool"]);
		assert!(items(&payload).iter().any(|item| item["type"] == "tool_search_call"));
		let output = items_of_type(&payload, "tool_search_output");
		assert_eq!(names(&output[0]["tools"]), ["late_tool"]);
	}

	#[test]
	fn folds_openai_updates_into_the_leading_developer_message_without_native_support() {
		let model = base_model(json!({
			"id": "gpt-4.1",
			"name": "GPT-4.1",
			"api": "openai-responses",
			"provider": "openai",
			"compat": { "supportsAdditionalTools": true },
		}));
		let payload = capture_payload(&model, tools_context());

		assert_eq!(names(&payload["tools"]), ["late_tool"]);
		let kinds: Vec<&str> = items(&payload)
			.iter()
			.map(|item| item.get("type").unwrap_or(&item["role"]).as_str().unwrap())
			.collect();
		assert_eq!(kinds, ["developer", "user"]);
		assert_eq!(
			items(&payload)[0]["content"],
			"base prompt\n\nupdated guidance\n\n<rules>\nnew rules\n</rules>"
		);
	}

	#[test]
	fn announces_openai_removals_as_text() {
		let payload = capture_payload(&additional_tools_model(), tools_context());

		assert!(payload.get("tools").is_none());
		let declared: Vec<&str> = items_of_type(&payload, "additional_tools")
			.iter()
			.map(|item| item["tools"][0]["name"].as_str().unwrap())
			.collect();
		assert_eq!(declared, ["base_tool", "late_tool"]);
		let update = content(items(&payload).last().unwrap());
		assert!(update.contains("updated guidance"), "{update}");
		assert!(
			update.contains("The following tools are no longer available. Do not call them:\n- base_tool"),
			"{update}"
		);
	}

	#[test]
	fn redefines_an_openai_tool_with_additional_tools_instead_of_announcing_a_removal() {
		let payload = capture_payload(
			&additional_tools_model(),
			json!({
				"messages": [
					{ "role": "system", "content": "base prompt", "toolsAdded": [base_tool()], "timestamp": 0 },
					{ "role": "user", "content": "before", "timestamp": 1 },
					{
						"role": "system",
						"content": "",
						"toolsRemoved": [{ "name": "base_tool" }],
						"toolsAdded": [tool_json(&described(tool("base_tool"), "changed"))],
						"timestamp": 2,
					},
				],
			}),
		);

		assert!(payload.get("tools").is_none());
		let input = items(&payload);
		assert_eq!(input.len(), 5);
		assert_match_object(
			&json!([&input[0], &input[1], &input[2], &input[4]]),
			&json!([
				{ "type": "additional_tools", "tools": [{ "name": "base_tool", "description": "base_tool tool" }] },
				{ "role": "developer", "content": "base prompt" },
				{ "role": "user" },
				{ "type": "additional_tools", "tools": [{ "name": "base_tool", "description": "changed" }] },
			]),
		);
		assert_eq!(input[3]["role"], "developer");
		assert!(content(&input[3]).contains("latest definition replaces the earlier one"));
	}

	// #10940: deferred loads after a removal must not rewrite the cached prefix.
	#[test]
	fn keeps_the_openai_request_prefix_across_a_removal_and_later_loads() {
		let payloads = capture_tool_history(&additional_tools_model());

		for payload in &payloads {
			assert!(payload.get("tools").is_none());
		}
		expect_growing_prefix(&payloads.iter().map(|payload| &payload["input"]).collect::<Vec<_>>());
		let last = payloads.last().unwrap();
		let declared: Vec<Vec<String>> = items_of_type(last, "additional_tools")
			.iter()
			.map(|item| names(&item["tools"]))
			.collect();
		assert_eq!(
			declared,
			[vec!["base_tool", "drop_tool"], vec!["late_tool"], vec!["later_tool"]]
		);
		assert!(
			items(last)
				.iter()
				.any(|item| content(item).contains("no longer available. Do not call them:\n- drop_tool"))
		);
		// Each later load restates the tools that are still unavailable.
		let notices: Vec<&str> = items(last)
			.iter()
			.map(content)
			.filter(|content| content.contains("incremental tools update"))
			.collect();
		assert_eq!(notices.len(), 2);
		for notice in notices {
			assert!(notice.contains("\nStill unavailable: drop_tool."), "{notice}");
		}
	}

	#[test]
	fn stops_listing_a_tool_as_unavailable_once_it_is_declared_again() {
		let payload = capture_payload(
			&additional_tools_model(),
			json!({
				"messages": [
					{ "role": "system", "content": "base prompt", "toolsAdded": [base_tool(), drop_tool()], "timestamp": 0 },
					{ "role": "system", "content": "", "toolsRemoved": [{ "name": "drop_tool" }], "timestamp": 1 },
					{ "role": "system", "content": "", "toolsAdded": [drop_tool()], "timestamp": 2 },
					{ "role": "user", "content": "go", "timestamp": 3 },
				],
			}),
		);

		let notice = items(&payload)
			.iter()
			.map(content)
			.find(|content| content.contains("incremental tools update"))
			.expect("notice is defined");
		assert!(!notice.contains("Still unavailable"), "{notice}");
		let input = items(&payload);
		assert_match_object(
			&input[input.len() - 2],
			&json!({ "type": "additional_tools", "tools": [{ "name": "drop_tool" }] }),
		);
	}

	#[test]
	fn does_not_list_a_tool_as_still_unavailable_before_its_removal_is_announced() {
		let payload = capture_payload(
			&additional_tools_model(),
			json!({
				"messages": [
					{ "role": "system", "content": "base prompt", "toolsAdded": [base_tool(), drop_tool()], "timestamp": 0 },
					{
						"role": "system",
						"content": "",
						"toolsAdded": [late_tool()],
						"toolsRemoved": [{ "name": "drop_tool" }],
						"timestamp": 1,
					},
					{ "role": "user", "content": "go", "timestamp": 2 },
				],
			}),
		);

		// Like Codex: notice, then the tools, then the removal notice.
		let input = items(&payload);
		let tail = &input[input.len() - 4..];
		assert!(content(&tail[0]).contains("incremental tools update"));
		assert!(!content(&tail[0]).contains("Still unavailable"));
		assert_match_object(
			&json!([&tail[1], { "role": &tail[3]["role"] }]),
			&json!([{ "type": "additional_tools", "tools": [{ "name": "late_tool" }] }, { "role": "user" }]),
		);
		assert!(content(&tail[2]).contains("no longer available. Do not call them:\n- drop_tool"));
	}

	// #10940: synthetic tool search cannot remove tools, so a removal rewrites the prefix once.
	#[test]
	fn keeps_the_openai_tool_search_prefix_across_loads_after_a_removal() {
		let mut model = additional_tools_model();
		model.compat = json!({ "supportsMidConvoSystemMessages": true, "supportsToolSearch": true })
			.as_object()
			.cloned();
		let payloads = capture_tool_history(&model);
		let (before_removal, after_removal) = payloads.split_first().unwrap();

		assert_eq!(names(&before_removal["tools"]), ["base_tool", "drop_tool"]);
		for payload in after_removal {
			assert_eq!(names(&payload["tools"]), ["base_tool"]);
		}
		expect_growing_prefix(
			&after_removal
				.iter()
				.map(|payload| &payload["input"])
				.collect::<Vec<_>>(),
		);
		let outputs: Vec<&str> = items_of_type(after_removal.last().unwrap(), "tool_search_output")
			.iter()
			.map(|item| item["tools"][0]["name"].as_str().unwrap())
			.collect();
		assert_eq!(outputs, ["late_tool", "later_tool"]);
	}
}

mod transcript_system_messages_openai_completions {
	use super::*;

	#[test]
	fn anchors_kimi_additions_in_tool_bearing_system_messages() {
		let model = base_model(json!({
			"id": "kimi-k3",
			"name": "Kimi K3",
			"api": "openai-completions",
			"provider": "moonshotai",
			"compat": { "supportsMidConvoSystemMessages": true, "supportsMidConvoToolAdditions": true },
		}));
		let payload = capture_payload(&model, addition_context());

		assert_eq!(function_names(&payload["tools"]), ["base_tool"]);
		let tool_message = messages(&payload)
			.iter()
			.find(|message| message.get("tools").is_some())
			.unwrap();
		assert_eq!(function_names(&tool_message["tools"]), ["late_tool"]);
		let system: Vec<Option<&str>> = messages(&payload)
			.iter()
			.filter(|message| message["role"] == "system")
			.map(|message| message.get("content").and_then(Value::as_str))
			.collect();
		assert_eq!(system, [Some("base prompt"), None, Some("updated guidance")]);
	}

	// #10940: Kimi tool messages cannot remove tools, so a removal rewrites the prefix once.
	#[test]
	fn keeps_the_kimi_prefix_across_loads_after_a_removal() {
		let payloads = capture_tool_history(&builtin("kimi-coding", "k3"));
		let (before_removal, after_removal) = payloads.split_first().unwrap();

		assert_eq!(function_names(&before_removal["tools"]), ["base_tool", "drop_tool"]);
		for payload in after_removal {
			assert_eq!(function_names(&payload["tools"]), ["base_tool"]);
		}
		expect_growing_prefix(
			&after_removal
				.iter()
				.map(|payload| &payload["messages"])
				.collect::<Vec<_>>(),
		);
		let loaded: Vec<&str> = messages(after_removal.last().unwrap())
			.iter()
			.filter(|message| message.get("tools").is_some())
			.map(|message| message["tools"][0]["function"]["name"].as_str().unwrap())
			.collect();
		assert_eq!(loaded, ["late_tool", "later_tool"]);
	}

	// #10720: Kimi Coding uses Chat Completions so K3-style models keep added tools out of the top-level list.
	#[test]
	fn anchors_additions_for_kimi_coding() {
		for model_id in ["k3", "k3-256k", "kimi-for-coding"] {
			let payload = capture_payload(&builtin("kimi-coding", model_id), addition_context());

			assert_eq!(function_names(&payload["tools"]), ["base_tool"], "{model_id}");
			let tool_message = messages(&payload)
				.iter()
				.find(|message| message.get("tools").is_some())
				.unwrap();
			assert_eq!(function_names(&tool_message["tools"]), ["late_tool"], "{model_id}");
		}
	}

	// #10720: Kimi Code Highspeed rejects tool-bearing system messages.
	#[test]
	fn keeps_kimi_coding_highspeed_additions_in_top_level_tools() {
		let payload = capture_payload(&builtin("kimi-coding", "kimi-for-coding-highspeed"), addition_context());

		assert_eq!(function_names(&payload["tools"]), ["base_tool", "late_tool"]);
		assert!(!messages(&payload).iter().any(|message| message.get("tools").is_some()));
	}

	#[test]
	fn keeps_kimi_k2_system_text_inline_without_dynamic_tool_messages() {
		let model = base_model(json!({
			"id": "kimi-k2.7-code",
			"name": "Kimi K2.7 Code",
			"api": "openai-completions",
			"provider": "moonshotai",
			"compat": { "supportsMidConvoSystemMessages": true },
		}));
		let payload = capture_payload(&model, addition_context());

		assert_eq!(function_names(&payload["tools"]), ["base_tool", "late_tool"]);
		assert!(!messages(&payload).iter().any(|message| message.get("tools").is_some()));
		let system: Vec<&Value> = messages(&payload)
			.iter()
			.filter(|message| message["role"] == "system")
			.map(|message| &message["content"])
			.collect();
		assert_eq!(system, [&json!("base prompt"), &json!("updated guidance")]);
	}

	#[test]
	fn folds_openai_compatible_updates_into_the_system_prompt_without_native_support() {
		let model = base_model(json!({
			"id": "custom-model",
			"name": "Custom model",
			"api": "openai-completions",
			"provider": "custom-provider",
			"reasoning": false,
		}));
		let payload = capture_payload(&model, tools_context());

		assert_eq!(function_names(&payload["tools"]), ["late_tool"]);
		assert_eq!(roles(&payload), ["system", "user"]);
		assert_eq!(
			messages(&payload)[0]["content"],
			"base prompt\n\nupdated guidance\n\n<rules>\nnew rules\n</rules>"
		);
	}
}
