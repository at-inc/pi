//! Port of `packages/ai/test/transcript-tool-changes.test.ts`.
//!
//! Ported: "resolves the top-level tool boundary per transport" (`resolveTranscriptTools` alone) and the Anthropic
//! Messages cases. The other tests capture a request payload through `streamSimple` and check how the OpenAI
//! Responses and OpenAI Completions (Kimi) APIs serialize tool changes; those move with those API shards.

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
