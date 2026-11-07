//! Port of `packages/ai/test/mistral-http-transport.test.ts`.
//!
//! The `fetch` option is a `FetchFunction` that records the `HttpRequest` it receives; `new Headers(init.headers)`
//! is `RecordedHeaders`, whose lookups ignore case like `Headers.get` / `Headers.has`.

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::mistral_conversations::{MistralOptions, stream};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{
	AssistantContent, AssistantMessage, Context, Model, OnPayload, OnProviderStreamEvent, OnResponse,
	ProviderRequestOptions, ProviderResponse, StopReason, StreamOptions, TextContent, ThinkingContent, ToolCall,
	TranscriptContext,
};
use pi_ai::utils::identity::format_user_agent;
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::json_stringify;
use pi_host::testing::run;
use pi_host::{AbortController, Body, FetchFunction, HttpRequest, HttpResponse};
use serde_json::{Value, json};

fn context(value: Value) -> TranscriptContext {
	normalize_context(&serde_json::from_value::<Context>(value).expect("valid context"))
}

fn hello_context() -> TranscriptContext {
	context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }))
}

fn mistral_model(id: &str) -> Model {
	get_builtin_model("mistral", id).expect("built-in model")
}

fn response(body: Body, headers: &[(&str, &str)]) -> HttpResponse {
	let mut response = HttpResponse::from_bytes(
		200,
		headers
			.iter()
			.map(|(name, value)| (name.to_string(), value.to_string()))
			.collect(),
		Vec::new(),
	);
	response.body = body;
	response
}

/// `createSseResponse(events, headers)`.
fn create_sse_response(events: &[Value], headers: &[(&str, &str)]) -> HttpResponse {
	let body = format!(
		"{}\r\n\r\ndata: [DONE]\r\n\r\n",
		events
			.iter()
			.map(|event| format!("data: {}", json_stringify(event)))
			.collect::<Vec<_>>()
			.join("\r\n\r\n")
	);
	let mut all_headers = vec![("content-type", "text/event-stream")];
	all_headers.extend_from_slice(headers);
	response(
		futures::stream::once(async move { Ok(body.into()) }).boxed_local(),
		&all_headers,
	)
}

/// `createBytewiseSseResponse(event)`.
fn create_bytewise_sse_response(event: &Value) -> HttpResponse {
	let bytes = format!("data: {}\r\n\r\ndata: [DONE]\r\n\r\n", json_stringify(event)).into_bytes();
	response(
		futures::stream::iter(bytes.into_iter().map(|byte| Ok(vec![byte].into()))).boxed_local(),
		&[("content-type", "text/event-stream")],
	)
}

fn create_terminal_event() -> Value {
	json!({
		"id": "mistral-response-id",
		"model": "mistral-large-latest",
		"choices": [{ "index": 0, "finish_reason": "stop", "delta": {} }],
		"usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 },
	})
}

/// A fetch that records each request and answers with `respond`.
fn recording_fetch(respond: impl Fn() -> HttpResponse + 'static) -> (FetchFunction, Rc<RefCell<Option<HttpRequest>>>) {
	let recorded: Rc<RefCell<Option<HttpRequest>>> = Rc::default();
	let target = recorded.clone();
	let fetch: FetchFunction = Rc::new(move |request| {
		*target.borrow_mut() = Some(request);
		let response = respond();
		Box::pin(async move { Ok(response) })
	});
	(fetch, recorded)
}

fn options(api_key: &str, fetch: FetchFunction) -> MistralOptions {
	MistralOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some(api_key.into()),
				fetch: Some(fetch),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		..MistralOptions::default()
	}
}

async fn result(model: &Model, context: &TranscriptContext, options: MistralOptions) -> AssistantMessage {
	stream(model, context, Some(options))
		.result()
		.await
		.expect("stream result")
}

/// `new Headers(init.headers)`.
struct RecordedHeaders(IndexMap<String, String>);

impl RecordedHeaders {
	fn get(&self, name: &str) -> Option<&str> {
		self.0
			.iter()
			.find(|(key, _)| key.eq_ignore_ascii_case(name))
			.map(|(_, value)| value.as_str())
	}

	fn has(&self, name: &str) -> bool {
		self.get(name).is_some()
	}
}

fn wire_payload(request: &HttpRequest) -> Value {
	serde_json::from_slice(request.body.as_deref().expect("request has a body")).expect("body is JSON")
}

fn text(text: &str) -> AssistantContent {
	AssistantContent::Text(TextContent {
		text: text.into(),
		..TextContent::default()
	})
}

fn thinking(thinking: &str) -> AssistantContent {
	AssistantContent::Thinking(ThinkingContent {
		thinking: thinking.into(),
		..ThinkingContent::default()
	})
}

fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantContent {
	let Value::Object(arguments) = arguments else {
		panic!("tool call arguments are an object");
	};
	AssistantContent::ToolCall(ToolCall {
		id: id.into(),
		name: name.into(),
		arguments,
		..ToolCall::default()
	})
}

mod mistral_http_transport {
	use super::*;

	#[test]
	fn serializes_sdk_style_payloads_to_the_mistral_wire_format() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let context = context(json!({
				"systemPrompt": "Be precise",
				"messages": [{
					"role": "user",
					"content": [
						{ "type": "text", "text": "describe" },
						{ "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" },
					],
					"timestamp": 1,
				}],
				"tools": [{
					"name": "lookup",
					"description": "Look something up",
					"parameters": {
						"type": "object",
						"properties": { "query": { "type": "string" } },
						"required": ["query"],
					},
				}],
			}));
			let (fetch, request) =
				recording_fetch(|| create_sse_response(&[create_terminal_event()], &[("x-request-id", "request-1")]));
			let callback_payload: Rc<RefCell<Option<Value>>> = Rc::default();
			let callback_response: Rc<RefCell<Option<ProviderResponse>>> = Rc::default();
			let payload_target = callback_payload.clone();
			let on_payload: OnPayload = Rc::new(move |payload, _model| {
				*payload_target.borrow_mut() = Some(payload.clone());
				let mut next = payload;
				let next_object = next.as_object_mut().expect("payload is an object");
				next_object.insert("topP".into(), json!(0.9));
				next_object.insert("randomSeed".into(), json!(42));
				next_object.insert(
					"responseFormat".into(),
					json!({
						"type": "json_schema",
						"jsonSchema": {
							"name": "result",
							"schemaDefinition": { "type": "object", "properties": { "maxTokens": { "type": "number" } } },
						},
					}),
				);
				next_object.insert("presencePenalty".into(), json!(0.1));
				next_object.insert("frequencyPenalty".into(), json!(0.2));
				next_object.insert("parallelToolCalls".into(), json!(true));
				next_object.insert("safePrompt".into(), json!(true));
				Box::pin(async move { Ok(Some(next)) })
			});
			let response_target = callback_response.clone();
			let on_response: OnResponse = Rc::new(move |response, _model| {
				*response_target.borrow_mut() = Some(response);
				Box::pin(async { Ok(()) })
			});
			let mut options = options("secret", fetch);
			options.headers = Some(IndexMap::from([("x-custom".to_string(), Some("value".to_string()))]));
			options.max_tokens = Some(123.0);
			options.prompt_mode = Some("reasoning".into());
			options.reasoning_effort = Some("high".into());
			options.tool_choice = Some(json!({ "type": "function", "function": { "name": "lookup" } }));
			options.session_id = Some("session-1".into());
			options.on_payload = Some(on_payload);
			options.on_response = Some(on_response);

			let message = result(&model, &context, options).await;

			assert_eq!(message.stop_reason, StopReason::Stop);
			let request = request.borrow().clone().expect("request sent");
			assert_eq!(request.url, "https://api.mistral.ai/v1/chat/completions");
			let headers = RecordedHeaders(request.headers.clone());
			assert_eq!(headers.get("authorization"), Some("Bearer secret"));
			assert_eq!(headers.get("accept"), Some("text/event-stream"));
			assert_eq!(headers.get("x-affinity"), Some("session-1"));
			assert_eq!(headers.get("x-custom"), Some("value"));
			assert_eq!(headers.get("user-agent"), Some(format_user_agent(None).as_str()));
			let callback_payload = callback_payload.borrow().clone().expect("payload captured");
			assert_eq!(callback_payload["maxTokens"].as_f64(), Some(123.0));
			assert_eq!(callback_payload["promptMode"], json!("reasoning"));
			assert_eq!(callback_payload["promptCacheKey"], json!("session-1"));
			assert_eq!(
				*callback_response.borrow(),
				Some(ProviderResponse {
					status: 200,
					headers: IndexMap::from([
						("content-type".to_string(), "text/event-stream".to_string()),
						("x-request-id".to_string(), "request-1".to_string()),
					]),
				})
			);

			let wire_payload = wire_payload(&request);
			assert_eq!(wire_payload["max_tokens"], json!(123));
			assert_eq!(wire_payload["prompt_mode"], json!("reasoning"));
			assert_eq!(wire_payload["reasoning_effort"], json!("high"));
			assert_eq!(
				wire_payload["tool_choice"],
				json!({ "type": "function", "function": { "name": "lookup" } })
			);
			assert_eq!(wire_payload["prompt_cache_key"], json!("session-1"));
			assert_eq!(wire_payload["top_p"], json!(0.9));
			assert_eq!(wire_payload["random_seed"], json!(42));
			assert_eq!(wire_payload["presence_penalty"], json!(0.1));
			assert_eq!(wire_payload["frequency_penalty"], json!(0.2));
			assert_eq!(wire_payload["parallel_tool_calls"], json!(true));
			assert_eq!(wire_payload["safe_prompt"], json!(true));
			assert_eq!(
				wire_payload["response_format"],
				json!({
					"type": "json_schema",
					"json_schema": {
						"name": "result",
						"schema": { "type": "object", "properties": { "maxTokens": { "type": "number" } } },
					},
				})
			);
			assert!(wire_payload.get("maxTokens").is_none());
			assert!(wire_payload.get("promptMode").is_none());
			assert!(wire_payload.get("promptCacheKey").is_none());
			assert_eq!(
				wire_payload["messages"],
				json!([
					{ "role": "system", "content": "Be precise" },
					{
						"role": "user",
						"content": [
							{ "type": "text", "text": "describe" },
							{ "type": "image_url", "image_url": "data:image/png;base64,aGVsbG8=" },
						],
					},
				])
			);
		});
	}

	#[test]
	fn serializes_assistant_thinking_tool_calls_and_tool_results_for_replay() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let context = context(json!({
				"messages": [
					{
						"role": "assistant",
						"api": "mistral-conversations",
						"provider": "mistral",
						"model": model.id,
						"content": [
							{ "type": "thinking", "thinking": "reason" },
							{ "type": "text", "text": "answer" },
							{ "type": "toolCall", "id": "abc123456", "name": "lookup", "arguments": { "query": "pi" } },
						],
						"usage": {
							"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
							"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
						},
						"stopReason": "toolUse",
						"timestamp": 1,
					},
					{
						"role": "toolResult",
						"toolCallId": "abc123456",
						"toolName": "lookup",
						"content": [
							{ "type": "text", "text": "found" },
							{ "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" },
						],
						"isError": false,
						"timestamp": 2,
					},
				],
			}));
			let (fetch, request) = recording_fetch(|| create_sse_response(&[create_terminal_event()], &[]));

			let message = result(&model, &context, options("test", fetch)).await;

			assert_eq!(message.stop_reason, StopReason::Stop);
			let request = request.borrow().clone().expect("request sent");
			assert_eq!(
				wire_payload(&request)["messages"],
				json!([
					{
						"role": "assistant",
						"prefix": false,
						"content": [
							{ "type": "thinking", "thinking": [{ "type": "text", "text": "reason" }] },
							{ "type": "text", "text": "answer" },
						],
						"tool_calls": [{
							"id": "abc123456",
							"type": "function",
							"function": { "name": "lookup", "arguments": "{\"query\":\"pi\"}" },
							"index": 0,
						}],
					},
					{
						"role": "tool",
						"tool_call_id": "abc123456",
						"name": "lookup",
						"content": [
							{ "type": "text", "text": "found" },
							{ "type": "image_url", "image_url": "data:image/png;base64,aGVsbG8=" },
						],
					},
				])
			);
		});
	}

	#[test]
	fn parses_native_thinking_text_fragmented_tool_calls_and_cached_token_usage() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let id = model.id.clone();
			let events = vec![
				json!({
					"id": "response-1", "model": id,
					"choices": [{ "index": 0, "finish_reason": null,
						"delta": { "content": [{ "type": "thinking", "thinking": [{ "type": "text", "text": "reason" }] }] } }],
				}),
				json!({
					"id": "response-1", "model": id,
					"choices": [{ "index": 0, "finish_reason": null,
						"delta": { "content": [{ "type": "text", "text": "answer" }] } }],
				}),
				json!({
					"id": "response-1", "model": id,
					"choices": [{ "index": 0, "finish_reason": null,
						"delta": { "tool_calls": [{ "id": "abc123456", "index": 0,
							"function": { "name": "lookup", "arguments": "{\"query\":" } }] } }],
				}),
				json!({
					"id": "response-1", "model": id,
					"choices": [{ "index": 0, "finish_reason": "tool_calls",
						"delta": { "tool_calls": [{ "index": 0, "function": { "name": "", "arguments": "\"pi\"}" } }] } }],
					"usage": {
						"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14,
						"prompt_tokens_details": { "cached_tokens": 3 },
					},
				}),
			];
			let (fetch, _request) = recording_fetch(move || create_sse_response(&events, &[]));

			let message = result(&model, &hello_context(), options("test", fetch)).await;

			assert_eq!(message.stop_reason, StopReason::ToolUse);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("tool_calls"));
			assert_eq!(message.response_id.as_deref(), Some("response-1"));
			assert_eq!(
				message.content,
				vec![
					thinking("reason"),
					text("answer"),
					tool_call("abc123456", "lookup", json!({ "query": "pi" })),
				]
			);
			assert_eq!(message.usage.input, 7.0);
			assert_eq!(message.usage.output, 4.0);
			assert_eq!(message.usage.cache_read, 3.0);
			assert_eq!(message.usage.cache_write, 0.0);
			assert_eq!(message.usage.total_tokens, 14.0);
		});
	}

	// #9674: GLM models on Mistral send empty content deltas at the start, around tool calls,
	// and sometimes mid-thinking. They must not open blocks or split thinking.
	#[test]
	fn ignores_empty_content_deltas() {
		run(|_host| async move {
			let model = mistral_model("zai-glm-5-3");
			let thinking_chunk =
				|text: &str| json!({ "type": "thinking", "thinking": [{ "type": "text", "text": text }] });
			let tool_call_delta = |args: &str, first: bool| {
				let mut call = json!({ "index": 0 });
				if first {
					call["id"] = json!("abc123456");
				}
				call["function"] = json!({ "name": if first { "read" } else { "" }, "arguments": args });
				call
			};
			let deltas = [
				json!({ "content": "" }),
				json!({ "content": [thinking_chunk("first part,")] }),
				json!({ "content": "" }),
				json!({ "content": [{ "type": "text", "text": "" }] }),
				json!({ "content": [thinking_chunk(" second part."), { "type": "text", "text": "Reading." }] }),
				json!({ "content": "", "tool_calls": [tool_call_delta("", true)] }),
				json!({ "content": "", "tool_calls": [tool_call_delta("{\"path\":", false)] }),
				json!({ "content": "", "tool_calls": [tool_call_delta("\"a.txt\"}", false)] }),
				json!({ "content": "" }),
			];
			let last = deltas.len() - 1;
			let events: Vec<Value> = deltas
				.into_iter()
				.enumerate()
				.map(|(i, delta)| {
					json!({
						"id": "response-1",
						"model": model.id,
						"choices": [{ "index": 0, "finish_reason": if i == last { json!("tool_calls") } else { Value::Null }, "delta": delta }],
					})
				})
				.collect();
			let (fetch, _request) = recording_fetch(move || create_sse_response(&events, &[]));

			let message = result(&model, &hello_context(), options("test", fetch)).await;

			assert_eq!(
				message.content,
				vec![
					thinking("first part, second part."),
					text("Reading."),
					tool_call("abc123456", "read", json!({ "path": "a.txt" })),
				]
			);
		});
	}

	#[test]
	fn forwards_each_parsed_sse_payload_before_normalizing_it() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let events = vec![
				json!({
					"id": "response-1",
					"provider_metadata": { "request": "test" },
					"choices": [{ "index": 0, "finish_reason": null, "delta": { "content": "hello" } }],
				}),
				json!({
					"id": "response-1",
					"choices": [{ "index": 0, "finish_reason": "stop", "delta": {} }],
					"usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 },
				}),
			];
			let served = events.clone();
			let (fetch, _request) = recording_fetch(move || create_sse_response(&served, &[]));
			let received: Rc<RefCell<Vec<Value>>> = Rc::default();
			let event_models: Rc<RefCell<Vec<Model>>> = Rc::default();
			let (received_in, event_models_in) = (received.clone(), event_models.clone());
			let on_provider_stream_event: OnProviderStreamEvent = Rc::new(move |event, event_model| {
				let (received, event_models, event, event_model) = (
					received_in.clone(),
					event_models_in.clone(),
					event.clone(),
					event_model.clone(),
				);
				Box::pin(async move {
					futures::future::ready(()).await;
					received.borrow_mut().push(event);
					event_models.borrow_mut().push(event_model);
					Ok(())
				})
			});
			let mut options = options("test", fetch);
			options.on_provider_stream_event = Some(on_provider_stream_event);

			let result = result(&model, &hello_context(), options).await;

			assert_eq!(*received.borrow(), events);
			assert_eq!(*event_models.borrow(), vec![model.clone(), model.clone()]);
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(result.content, vec![text("hello")]);
		});
	}

	#[test]
	fn parses_sse_and_utf_8_sequences_split_across_transport_chunks() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let event = json!({
				"id": "response-bytewise",
				"model": model.id,
				"choices": [{ "index": 0, "finish_reason": "stop", "delta": { "content": "héllo 🌍" } }],
				"usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 },
			});
			let (fetch, _request) = recording_fetch(move || create_bytewise_sse_response(&event));

			let message = result(&model, &hello_context(), options("test", fetch)).await;

			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(message.content, vec![text("héllo 🌍")]);
		});
	}

	#[test]
	fn honors_case_insensitive_header_overrides_and_explicit_affinity_suppression() {
		run(|_host| async move {
			let mut model = mistral_model("mistral-large-latest");
			model.headers = Some(IndexMap::from([
				("Authorization".to_string(), "Bearer model-key".to_string()),
				("X-Affinity".to_string(), "model-affinity".to_string()),
			]));
			let (fetch, request) = recording_fetch(|| create_sse_response(&[create_terminal_event()], &[]));
			let mut options = options("request-key", fetch);
			options.session_id = Some("automatic-affinity".into());
			options.headers = Some(IndexMap::from([
				("authorization".to_string(), None),
				("x-affinity".to_string(), None),
				("User-Agent".to_string(), Some("custom-agent".to_string())),
			]));

			result(&model, &hello_context(), options).await;

			let headers = RecordedHeaders(request.borrow().clone().expect("request sent").headers);
			assert!(!headers.has("authorization"));
			assert!(!headers.has("x-affinity"));
			assert_eq!(headers.get("user-agent"), Some("custom-agent"));
		});
	}

	#[test]
	fn aborts_while_waiting_for_an_sse_chunk() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let controller = AbortController::new();
			let (fetch, _request) = recording_fetch(|| {
				response(
					futures::stream::pending().boxed_local(),
					&[("content-type", "text/event-stream")],
				)
			});
			let mut options = options("test", fetch);
			options.signal = Some(controller.signal());

			let stream = stream(&model, &hello_context(), Some(options));
			let result = stream.result();
			let aborter = async {
				futures::future::ready(()).await;
				controller.abort();
			};
			let (message, ()) = futures::join!(result, aborter);

			assert_eq!(message.expect("stream result").stop_reason, StopReason::Aborted);
		});
	}

	#[test]
	fn applies_the_request_timeout_while_waiting_for_response_headers() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let fetch: FetchFunction = Rc::new(|request: HttpRequest| {
				Box::pin(async move {
					let signal = request.signal.expect("request has a signal");
					Err(signal.aborted_future().await)
				})
			});
			let mut options = options("test", fetch);
			options.timeout_ms = Some(5.0);

			let message = result(&model, &hello_context(), options).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(
				message.error_message.as_deref(),
				Some("Mistral response headers timed out after 5ms")
			);
		});
	}

	// Regression test for #10609: an active stream must not be cut off after timeoutMs.
	#[test]
	fn does_not_abort_an_active_stream_that_lasts_longer_than_the_request_timeout() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let thinking_event = json!({
				"choices": [{ "index": 0, "delta": { "content": [{ "type": "thinking", "thinking": [{ "type": "text", "text": "x" }] }] } }],
			});
			let (fetch, _request) = recording_fetch(move || {
				let thinking = format!("data: {}\n\n", json_stringify(&thinking_event));
				let terminal = format!("data: {}\n\ndata: [DONE]\n\n", json_stringify(&create_terminal_event()));
				let body = futures::stream::unfold(0, move |i| {
					let (thinking, terminal) = (thinking.clone(), terminal.clone());
					async move {
						match i {
							0..5 => {
								if i > 0 {
									pi_host::sleep(10.0, None).await.expect("sleep");
								}
								Some((Ok(thinking.into()), i + 1))
							}
							5 => {
								pi_host::sleep(10.0, None).await.expect("sleep");
								Some((Ok(terminal.into()), i + 1))
							}
							_ => None,
						}
					}
				});
				response(body.boxed_local(), &[("content-type", "text/event-stream")])
			});
			let mut options = options("test", fetch);
			options.timeout_ms = Some(20.0);

			let message = result(&model, &hello_context(), options).await;

			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(message.content, vec![thinking("xxxxx")]);
		});
	}

	#[test]
	fn preserves_http_status_and_response_bodies_in_errors() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let (fetch, _request) = recording_fetch(|| {
				let mut response =
					HttpResponse::from_bytes(403, IndexMap::new(), r#"{"message":"blocked by gateway"}"#);
				response.status_text = "Forbidden".into();
				response
			});

			let message = result(&model, &hello_context(), options("test", fetch)).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(
				message.error_message.as_deref(),
				Some(r#"Mistral API error (403): {"message":"blocked by gateway"}"#)
			);
		});
	}

	// Not in the TS suite: the TypeErrors V8 reports for a null or missing `function` and tool choice function, checked
	// against the TS with an oracle script.
	#[test]
	fn reports_v8_type_errors_for_null_or_missing_functions() {
		run(|_host| async move {
			let model = mistral_model("mistral-large-latest");
			let cases = [
				(
					vec![json!({ "index": 0, "function": null })],
					"Cannot read properties of null (reading 'name')",
				),
				(
					vec![
						json!({ "index": 0, "function": { "name": "get", "arguments": "{" } }),
						json!({ "index": 0 }),
					],
					"Cannot read properties of undefined (reading 'arguments')",
				),
				(
					vec![
						json!({ "index": 0, "function": { "name": "get", "arguments": "{" } }),
						json!({ "index": 0, "function": null }),
					],
					"Cannot read properties of null (reading 'arguments')",
				),
			];
			for (tool_calls, expected) in cases {
				let events: Vec<Value> = tool_calls
					.into_iter()
					.map(|tool_call| json!({ "choices": [{ "delta": { "tool_calls": [tool_call] } }] }))
					.collect();
				let (fetch, _request) = recording_fetch(move || create_sse_response(&events, &[]));
				let message = result(&model, &hello_context(), options("test", fetch)).await;
				assert_eq!(message.error_message.as_deref(), Some(expected));
			}

			for (tool_choice, expected) in [
				(
					json!({ "type": "function", "function": null }),
					"Cannot read properties of null (reading 'name')",
				),
				(
					json!({ "type": "function" }),
					"Cannot read properties of undefined (reading 'name')",
				),
				(json!(5), "Cannot read properties of undefined (reading 'name')"),
			] {
				let (fetch, request) = recording_fetch(|| create_sse_response(&[create_terminal_event()], &[]));
				let message = result(
					&model,
					&hello_context(),
					MistralOptions {
						tool_choice: Some(tool_choice),
						..options("test", fetch)
					},
				)
				.await;
				assert_eq!(message.error_message.as_deref(), Some(expected));
				assert!(request.borrow().is_none());
			}
		});
	}
}
