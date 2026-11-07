//! Port of `packages/agent/test/malformed-tool-arguments.test.ts`.
//!
//! The TS test answers the direct requests with pi-ai's faux provider through `createModels().streamSimple`.
//! `providers/faux.rs` and `models.rs` are still pending in shard w2-ai-core, so `faux_stream_fn` below stands in for
//! them: it answers each request with the next scripted message, built like `fauxAssistantMessage` and
//! `fauxToolCall` build them. The proxy requests go through `streamProxy` and a stubbed `fetch`, as in TS.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;
use pi_agent::agent::{Agent, AgentInitialState, AgentOptions};
use pi_agent::proxy::{ProxyStreamOptions, stream_proxy};
use pi_agent::types::{AgentMessage, AgentTool, AgentToolResult, StreamFn};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Message, Model, StopReason, TextContent, Tool,
	ToolCall, UserContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::HttpResponse;
use pi_host::testing::{TestHost, run};
use pi_schema::builders::{Property, obj};
use serde_json::{Map, Value, json};

/// `fauxProvider().getModel()`.
fn faux_model() -> Model {
	serde_json::from_value(json!({
		"id": "faux-1",
		"name": "Faux Model",
		"api": "faux",
		"provider": "faux",
		"baseUrl": "http://localhost:0",
		"reasoning": false,
		"input": ["text", "image"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 128000,
		"maxTokens": 16384,
	}))
	.unwrap()
}

/// `fauxToolCall(name, arguments, { id })`.
fn faux_tool_call(name: &str, arguments: Map<String, Value>, id: &str) -> ToolCall {
	ToolCall {
		id: id.into(),
		name: name.into(),
		arguments,
		..ToolCall::default()
	}
}

/// `fauxAssistantMessage(content, { stopReason })`.
fn faux_assistant_message(content: Vec<AssistantContent>, stop_reason: StopReason) -> AssistantMessage {
	AssistantMessage {
		content,
		api: "faux".into(),
		provider: "faux".into(),
		model: "faux-1".into(),
		stop_reason,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

fn faux_text_message(text: &str) -> AssistantMessage {
	faux_assistant_message(
		vec![AssistantContent::Text(TextContent {
			text: text.into(),
			..TextContent::default()
		})],
		StopReason::Stop,
	)
}

/// The faux provider's `streamSimple` with `setResponses(responses)`: each request answers with the next response.
fn faux_stream(responses: &RefCell<Vec<AssistantMessage>>) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let message = responses.borrow_mut().remove(0);
	let output = stream.clone();
	pi_host::spawn_local(async move {
		let reason = message.stop_reason;
		output.push(AssistantMessageEvent::Done {
			reason,
			message: Rc::new(RefCell::new(message)),
		});
	});
	stream
}

fn stub_fetch(host: &TestHost, body: String) {
	host.on_fetch(move |_request| {
		let body = body.clone();
		Box::pin(async move { Ok(HttpResponse::from_bytes(200, IndexMap::new(), body)) })
	});
}

async fn never_executes_through(host: Rc<TestHost>, transport: &'static str, prepare: bool) {
	let mut call = faux_tool_call("unsafe", Map::new(), "malformed-call");
	call.arguments_error = Some("Invalid JSON object: {\"value\":".into());
	let first = faux_assistant_message(vec![AssistantContent::ToolCall(call.clone())], StopReason::ToolUse);
	let responses = Rc::new(RefCell::new(if transport == "direct" {
		vec![first.clone(), faux_text_message("done")]
	} else {
		vec![faux_text_message("done")]
	}));
	let execute_calls = Rc::new(Cell::new(0));
	let prepare_calls = Rc::new(Cell::new(0));
	let parameters = obj(Vec::<(&str, Property)>::new(), Value::Null);
	let tool = AgentTool {
		tool: Tool {
			name: "unsafe".into(),
			description: "Must not execute malformed arguments".into(),
			parameters,
			..Tool::default()
		},
		label: "Unsafe".into(),
		prepare_arguments: prepare.then(|| {
			let prepare_calls = prepare_calls.clone();
			Rc::new(move |_args: Value| {
				prepare_calls.set(prepare_calls.get() + 1);
				Ok(Map::new())
			}) as pi_agent::types::PrepareArgumentsFn
		}),
		output_schema: None,
		execute: {
			let execute_calls = execute_calls.clone();
			Rc::new(move |_id, _params, _signal, _on_update| {
				execute_calls.set(execute_calls.get() + 1);
				Box::pin(async {
					Ok(AgentToolResult {
						content: vec![UserContent::Text(TextContent {
							text: "unexpected".into(),
							..TextContent::default()
						})],
						details: Some(json!({})),
						..AgentToolResult::default()
					})
				})
			})
		},
		replay: None,
		execution_mode: None,
	};
	let requests = Rc::new(Cell::new(0));
	let stream_fn: StreamFn = {
		let requests = requests.clone();
		let host = host.clone();
		let call = call.clone();
		let usage = first.usage.clone();
		Rc::new(move |model, context, _options| {
			requests.set(requests.get() + 1);
			if transport == "direct" || requests.get() > 1 {
				let stream = faux_stream(&responses);
				return Box::pin(async move { Ok(stream) });
			}
			let events = [
				json!({ "type": "start" }),
				json!({ "type": "toolcall_start", "contentIndex": 0, "id": call.id, "toolName": call.name }),
				json!({ "type": "toolcall_delta", "contentIndex": 0, "delta": "{\"value\":" }),
				json!({ "type": "toolcall_end", "contentIndex": 0, "toolCall": call }),
				json!({ "type": "done", "reason": "toolUse", "usage": usage }),
			];
			stub_fetch(
				&host,
				events
					.iter()
					.map(|event| format!("data: {}\n\n", pi_host::js::json_stringify(event)))
					.collect(),
			);
			let stream = stream_proxy(
				model,
				context,
				ProxyStreamOptions {
					auth_token: "test-token".into(),
					proxy_url: "https://proxy.example".into(),
					..ProxyStreamOptions::default()
				},
			);
			Box::pin(async move { Ok(stream) })
		})
	};
	let before_tool_calls = Rc::new(Cell::new(0));
	let agent = Agent::new(AgentOptions {
		initial_state: Some(AgentInitialState {
			model: Some(faux_model()),
			tools: Some(vec![tool]),
			..AgentInitialState::default()
		}),
		stream_fn: Some(stream_fn),
		before_tool_call: Some({
			let before_tool_calls = before_tool_calls.clone();
			Rc::new(move |_context, _signal| {
				before_tool_calls.set(before_tool_calls.get() + 1);
				Box::pin(async { Ok(None) })
			})
		}),
		..AgentOptions::default()
	})
	.unwrap();
	agent.prompt("Attempt the tool call.").await.unwrap();

	assert_eq!(execute_calls.get(), 0);
	assert_eq!(before_tool_calls.get(), 0);
	assert_eq!(prepare_calls.get(), if prepare { 1 } else { 0 });
	assert_eq!(requests.get(), 2);
	assert_eq!(agent.state().error_message(), None);
	let state = agent.state();
	let tool_result = state
		.messages()
		.iter()
		.find_map(|message| match message {
			AgentMessage::Llm(Message::ToolResult(result)) => Some(result),
			_ => None,
		})
		.expect("a tool result");
	assert!(tool_result.is_error);
	assert_eq!(tool_result.tool_call_id, call.id);
	let [UserContent::Text(text)] = tool_result.content.as_slice() else {
		panic!("expected one text block, got {:?}", tool_result.content);
	};
	assert!(text.text.contains(call.arguments_error.as_deref().unwrap()));
}

mod malformed_final_tool_arguments {
	use super::*;

	// Regression for https://github.com/earendil-works/pi/issues/10897.
	#[test]
	fn never_executes_through_direct_with_prepare_arguments_false() {
		run(|host| never_executes_through(host, "direct", false));
	}

	// Regression for https://github.com/earendil-works/pi/issues/10897.
	#[test]
	fn never_executes_through_direct_with_prepare_arguments_true() {
		run(|host| never_executes_through(host, "direct", true));
	}

	// Regression for https://github.com/earendil-works/pi/issues/10897.
	#[test]
	fn never_executes_through_proxy_with_prepare_arguments_false() {
		run(|host| never_executes_through(host, "proxy", false));
	}

	// Regression for https://github.com/earendil-works/pi/issues/10897.
	#[test]
	fn never_executes_through_proxy_with_prepare_arguments_true() {
		run(|host| never_executes_through(host, "proxy", true));
	}
}
