//! Port of `packages/ai/test/anthropic-strict-tool-fallback.test.ts`.
//!
//! Differences from the TS:
//! - The `fetch` option of the harness is a `FetchFunction` that parses each request body and answers it; the
//!   responses go through the real client's HTTP error parsing, like the TS.
//! - `it.each` cases run in a loop. `vi.fn` spies are call counters. `Anthropic.APIError.generate(400, ...)` is the
//!   `PiError` the Rust client builds for that response: message `400 <JSON of the error body>`, status 400, and
//!   `data` carrying `headers`, `error` and `type`.
//! - `expect(JSON.stringify(transcript)).toBe(original)` holds by construction, since `stream` borrows the transcript
//!   immutably; the assertion is kept on the serialized transcript.
//!
//! Not ported: "preserves prefer through pi-messages so the gateway's Anthropic adapter can fall back". It streams
//! through `api::pi_messages`, which is still `PORT-STATUS: pending` (another w3 shard), so calling it would not
//! compile.

mod anthropic_support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anthropic_support::context;
use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream};
use pi_ai::types::{
	CacheRetention, Message, Model, ProviderRequestOptions, StopReason, StreamOptions, TranscriptContext,
};
use pi_host::js::json_stringify;
use pi_host::testing::run;
use pi_host::{AbortController, FetchFunction, HttpResponse, PiError};
use serde_json::{Value, json};

const GRAMMAR_ERROR: &str = "The compiled grammar is too large, which would cause performance issues. Simplify your tool schemas or reduce the number of strict tools.";

/// The `model` of the TS file.
fn model() -> Model {
	serde_json::from_value(json!({
		"id": "claude-haiku-4-5",
		"name": "Test Claude",
		"api": "anthropic-messages",
		"provider": "anthropic",
		"baseUrl": "https://anthropic.example",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 100,
		"compat": { "supportsStrictTools": true },
	}))
	.unwrap()
}

/// `tool(name, strict)`: `Type.Object({ value: Type.String(), optional: Type.Optional(Type.Number()) })` parameters.
fn tool(name: &str, strict: Option<&str>) -> Value {
	let mut tool = json!({
		"name": name,
		"description": "Look up a value",
		"parameters": {
			"type": "object",
			"required": ["value"],
			"properties": { "value": { "type": "string" }, "optional": { "type": "number" } },
		},
	});
	if let Some(strict) = strict {
		tool["constrainedSampling"] = json!({ "type": "json_schema", "strict": strict });
	}
	tool
}

/// `context(tools)`.
fn transcript(tools: Vec<Value>) -> TranscriptContext {
	context(json!({ "tools": tools, "messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }] }))
}

fn json_response(status: u16, body: String) -> HttpResponse {
	HttpResponse::from_bytes(
		status,
		IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
		body,
	)
}

/// `errorResponse(message, status, type)`.
fn error_response(message: &str, status: u16, r#type: &str) -> HttpResponse {
	json_response(
		status,
		json_stringify(&json!({ "type": "error", "error": { "type": r#type, "message": message } })),
	)
}

/// `errorResponse()` with its defaults.
fn grammar_error_response() -> HttpResponse {
	error_response(GRAMMAR_ERROR, 400, "invalid_request_error")
}

fn sse(body: String) -> HttpResponse {
	HttpResponse::from_bytes(
		200,
		IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
		body,
	)
}

/// `successResponse()`.
fn success_response() -> HttpResponse {
	let events = [
		json!({
			"type": "message_start",
			"message": { "id": "msg_test", "model": "claude-haiku-4-5", "usage": { "input_tokens": 10, "output_tokens": 0 } },
		}),
		json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 1 } }),
		json!({ "type": "message_stop" }),
	];
	sse(events
		.iter()
		.map(|event| {
			format!(
				"event: {}\ndata: {}\n\n",
				event["type"].as_str().unwrap(),
				json_stringify(event)
			)
		})
		.collect())
}

type Payloads = Rc<RefCell<Vec<Value>>>;

/// `harness(respond)`: the recorded payloads and `{ apiKey: "test-key", fetch, maxRetries: 0 }`.
fn harness(respond: impl Fn(&Value, usize) -> HttpResponse + 'static) -> (Payloads, AnthropicOptions) {
	let payloads: Payloads = Rc::default();
	let recorder = payloads.clone();
	let fetch: FetchFunction = Rc::new(move |request| {
		let payload: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
		recorder.borrow_mut().push(payload.clone());
		let response = respond(&payload, recorder.borrow().len());
		Box::pin(async move { Ok(response) })
	});
	let options = AnthropicOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("test-key".into()),
				fetch: Some(fetch),
				max_retries: Some(0.0),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		..AnthropicOptions::default()
	};
	(payloads, options)
}

/// `payload.tools[index]`.
fn tool_at(payload: &Value, index: usize) -> Value {
	payload["tools"][index].clone()
}

/// `expect(actual).toMatchObject(expected)`: every property of `expected` matches recursively; arrays match element
/// by element with the same length.
fn matches_object(actual: &Value, expected: &Value) -> bool {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => expected
			.iter()
			.all(|(key, expected)| actual.get(key).is_some_and(|actual| matches_object(actual, expected))),
		(Value::Array(actual), Value::Array(expected)) => {
			actual.len() == expected.len()
				&& actual
					.iter()
					.zip(expected)
					.all(|(actual, expected)| matches_object(actual, expected))
		}
		(Value::Number(actual), Value::Number(expected)) => actual.as_f64() == expected.as_f64(),
		(actual, expected) => actual == expected,
	}
}

fn assert_match_object(actual: &Value, expected: Value) {
	assert!(matches_object(actual, &expected), "{actual} does not match {expected}");
}

// Regression tests for https://github.com/earendil-works/pi/issues/10409.
mod anthropic_preferred_strict_tool_fallback {
	use super::*;

	#[test]
	fn retries_with_original_non_strict_schemas_and_preserves_required_tools() {
		for message in [
			GRAMMAR_ERROR,
			// Observed from the live API: 32 strict tools, then 20 tools with 24 optional fields each.
			"Too many strict tools (32). The maximum number of strict tools supported is 20. Try reducing the number of tools marked as strict.",
			"Schemas contains too many parameters with union types (480 parameters with type arrays or anyOf). This causes exponential compilation cost. Reduce the number of nullable or union-typed parameters (limit: 16 parameters with unions).",
		] {
			run(|_| async move {
				let preferred = tool("preferred", Some("prefer"));
				let required = tool("required", Some("require"));
				let ordinary = tool("ordinary", None);
				let transcript = transcript(vec![preferred, required, ordinary]);
				let original = json_stringify(&serde_json::to_value(&transcript).unwrap());
				// Use the real SDK's HTTP error parsing, not a synthetic thrown Error.
				let (payloads, mut options) = harness(move |_, attempt| {
					if attempt == 1 {
						error_response(message, 400, "invalid_request_error")
					} else {
						success_response()
					}
				});
				let on_payload_calls = Rc::new(Cell::new(0));
				let on_response_calls = Rc::new(Cell::new(0));
				let counter = on_payload_calls.clone();
				options.on_payload = Some(Rc::new(move |payload, _| {
					counter.set(counter.get() + 1);
					let mut payload = payload.as_object().unwrap().clone();
					payload.insert("inference_geo".into(), json!("us"));
					payload.insert("stream".into(), json!(false));
					Box::pin(async move { Ok(Some(Value::Object(payload))) })
				}));
				let counter = on_response_calls.clone();
				options.on_response = Some(Rc::new(move |_, _| {
					counter.set(counter.get() + 1);
					Box::pin(async { Ok(()) })
				}));
				options.cache_retention = Some(CacheRetention::Long);
				let stream = stream(&model(), &transcript, Some(options));
				let events: Vec<String> = stream
					.clone()
					.into_stream()
					.map(|event| {
						serde_json::to_value(&event).unwrap()["type"]
							.as_str()
							.unwrap()
							.to_string()
					})
					.collect()
					.await;
				let result = stream.result().await.unwrap();

				assert_eq!(result.stop_reason, StopReason::Stop, "{message}");
				assert_eq!(events, ["start", "done"]);
				let payloads = payloads.borrow();
				assert_eq!(payloads.len(), 2);
				assert_eq!(on_payload_calls.get(), 2);
				assert_eq!(on_response_calls.get(), 1);
				let (first, second) = (&payloads[0], &payloads[1]);
				assert_match_object(
					&tool_at(first, 0),
					json!({
						"strict": true,
						"input_schema": {
							"required": ["value", "optional"],
							"properties": { "optional": { "anyOf": [{ "type": "number" }, { "type": "null" }] } },
						},
					}),
				);
				assert_eq!(tool_at(second, 0).get("strict"), None);
				assert_eq!(
					tool_at(second, 0)["input_schema"],
					json!({
						"type": "object",
						"properties": { "value": { "type": "string" }, "optional": { "type": "number" } },
						"required": ["value"],
					})
				);
				assert_eq!(tool_at(second, 1), tool_at(first, 1));
				assert_eq!(tool_at(second, 1)["strict"], json!(true));
				assert_eq!(tool_at(second, 2), tool_at(first, 2));
				assert_eq!(
					tool_at(second, 2)["cache_control"],
					json!({ "type": "ephemeral", "ttl": "1h" })
				);
				assert_eq!(
					payloads
						.iter()
						.map(|payload| payload["stream"].clone())
						.collect::<Vec<_>>(),
					[json!(true), json!(true)]
				);
				assert_match_object(second, json!({ "inference_geo": "us" }));
				assert_eq!(json_stringify(&serde_json::to_value(&transcript).unwrap()), original);
				let diagnostics = serde_json::to_value(&result.diagnostics).unwrap();
				assert_match_object(
					&diagnostics[0],
					json!({
						"type": "anthropic_strict_tools_fallback",
						"details": { "reason": "strict_tool_limits_exceeded" },
					}),
				);
			});
		}
	}

	#[test]
	fn downgrades_inline_additions_and_same_name_redefinitions_without_downgrading_require() {
		run(|_| async {
			let mut transcript = transcript(vec![tool("lookup", Some("require"))]);
			for message in [
				json!({ "role": "system", "content": "", "toolsAdded": [tool("lookup", Some("prefer"))], "timestamp": 2 }),
				json!({ "role": "system", "content": "", "toolsAdded": [tool("lookup", Some("require"))], "timestamp": 3 }),
			] {
				transcript
					.messages
					.push(serde_json::from_value::<Message>(message).unwrap());
			}
			let (payloads, mut options) = harness(|_, attempt| {
				if attempt == 1 {
					grammar_error_response()
				} else {
					success_response()
				}
			});
			let mut model = model();
			let compat = model.compat.get_or_insert_default();
			compat.insert("supportsMidConvoSystemMessages".into(), json!(true));
			compat.insert("supportsMidConvoToolChanges".into(), json!(true));
			options.api_key = Some("sk-ant-oat-test".into());
			let result = stream(&model, &transcript, Some(options)).result().await.unwrap();
			assert_eq!(result.stop_reason, StopReason::Stop);
			let payloads = payloads.borrow();
			assert_eq!(payloads.len(), 2);
			assert_eq!(payloads[1]["tools"], payloads[0]["tools"]);
			let additions: Vec<Vec<Value>> = payloads
				.iter()
				.map(|payload| {
					payload["messages"]
						.as_array()
						.unwrap()
						.iter()
						.flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
						.filter(|block| block["type"] == "tool_addition" && block["tool"]["type"] == "tool_definition")
						.map(|block| block["tool"]["definition"].clone())
						.collect()
				})
				.collect();
			let stricts = |definitions: &[Value]| -> Vec<Option<Value>> {
				definitions
					.iter()
					.map(|definition| definition.get("strict").cloned())
					.collect()
			};
			assert_eq!(stricts(&additions[0]), [Some(json!(true)), Some(json!(true))]);
			assert_eq!(stricts(&additions[1]), [None, Some(json!(true))]);
			assert_eq!(additions[1][0]["input_schema"]["required"], json!(["value"]));
			assert_eq!(additions[1][1], additions[0][1]);
		});
	}

	#[test]
	fn does_not_retry_a_tool_set() {
		let mut locally_unsupported_prefer = tool("lookup", Some("prefer"));
		// Type.Object({ value: Type.Number({ minimum: 1 }) })
		locally_unsupported_prefer["parameters"] = json!({
			"type": "object",
			"required": ["value"],
			"properties": { "value": { "type": "number", "minimum": 1 } },
		});
		for (name, tools) in [
			("required", vec![tool("lookup", Some("require"))]),
			("ordinary", vec![tool("lookup", None)]),
			("locally unsupported prefer", vec![locally_unsupported_prefer]),
		] {
			run(|_| async move {
				let (payloads, options) = harness(|_, _| grammar_error_response());
				let result = stream(&model(), &transcript(tools), Some(options))
					.result()
					.await
					.unwrap();
				assert_eq!(result.stop_reason, StopReason::Error, "{name}");
				assert!(result.error_message.unwrap().contains(GRAMMAR_ERROR), "{name}");
				assert_eq!(payloads.borrow().len(), 1, "{name}");
			});
		}
	}

	#[test]
	fn does_not_downgrade_on() {
		for (name, message, status, r#type) in [
			("unrelated 400", "Invalid tool schema", 400, "invalid_request_error"),
			("wrong error type", GRAMMAR_ERROR, 400, "authentication_error"),
			("wrong HTTP status", GRAMMAR_ERROR, 500, "invalid_request_error"),
		] {
			run(|_| async move {
				let (payloads, options) = harness(move |_, _| error_response(message, status, r#type));
				let result = stream(
					&model(),
					&transcript(vec![tool("lookup", Some("prefer"))]),
					Some(options),
				)
				.result()
				.await
				.unwrap();
				assert_eq!(result.stop_reason, StopReason::Error, "{name}");
				assert_eq!(payloads.borrow().len(), 1, "{name}");
			});
		}
	}

	#[test]
	fn surfaces_a_second_rejection_rather_than_retrying_again() {
		run(|_| async {
			let (payloads, options) = harness(|_, attempt| {
				error_response(
					if attempt == 1 {
						GRAMMAR_ERROR
					} else {
						"Required grammar is still too large"
					},
					400,
					"invalid_request_error",
				)
			});
			let result = stream(
				&model(),
				&transcript(vec![tool("lookup", Some("prefer")), tool("required", Some("require"))]),
				Some(options),
			)
			.result()
			.await
			.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert!(
				result
					.error_message
					.unwrap()
					.contains("Required grammar is still too large")
			);
			let payloads = payloads.borrow();
			assert_eq!(payloads.len(), 2);
			assert_eq!(tool_at(&payloads[1], 1)["strict"], json!(true));
		});
	}

	#[test]
	fn does_not_downgrade_a_hook_exception_even_if_it_looks_like_the_provider_rejection() {
		run(|_| async {
			let (payloads, mut options) = harness(|_, _| success_response());
			options.on_payload = Some(Rc::new(|_, _| {
				// Anthropic.APIError.generate(400, { error: { type, message } }, grammarError, new Headers())
				let error_response = json!({ "error": { "type": "invalid_request_error", "message": GRAMMAR_ERROR } });
				let error = PiError::new(format!("400 {}", json_stringify(&error_response)))
					.with_status(400)
					.with_data(json!({ "headers": {}, "error": error_response, "type": "invalid_request_error" }));
				Box::pin(async move { Err(error) })
			}));
			let result = stream(
				&model(),
				&transcript(vec![tool("lookup", Some("prefer"))]),
				Some(options),
			)
			.result()
			.await
			.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(payloads.borrow().len(), 0);
			let diagnostics = serde_json::to_value(&result.diagnostics).unwrap();
			let diagnostics = diagnostics.as_array().unwrap();
			assert_eq!(diagnostics.len(), 1);
			assert_match_object(
				&diagnostics[0],
				json!({ "type": "provider_error", "details": { "status": 400 } }),
			);
			assert!(
				diagnostics[0]["error"]["message"]
					.as_str()
					.unwrap()
					.contains(GRAMMAR_ERROR)
			);
		});
	}

	#[test]
	fn honors_cancellation_before_the_fallback_attempt() {
		run(|_| async {
			let controller = AbortController::new();
			let aborter = controller.clone();
			let (payloads, mut options) = harness(move |_, _| {
				aborter.abort();
				grammar_error_response()
			});
			options.signal = Some(controller.signal());
			let result = stream(
				&model(),
				&transcript(vec![tool("lookup", Some("prefer"))]),
				Some(options),
			)
			.result()
			.await
			.unwrap();
			assert_eq!(result.stop_reason, StopReason::Aborted);
			assert_eq!(payloads.borrow().len(), 1);
		});
	}

	#[test]
	fn does_not_retry_an_sse_error_after_response_headers() {
		run(|_| async {
			let (payloads, options) = harness(|_, _| {
				sse(format!(
					"event: error\ndata: {}\n\n",
					json_stringify(
						&json!({ "type": "error", "error": { "type": "invalid_request_error", "message": GRAMMAR_ERROR } })
					)
				))
			});
			let result = stream(
				&model(),
				&transcript(vec![tool("lookup", Some("prefer"))]),
				Some(options),
			)
			.result()
			.await
			.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(payloads.borrow().len(), 1);
		});
	}

	#[test]
	fn does_not_disable_strict_sampling_on_later_requests() {
		run(|_| async {
			let (payloads, options) = harness(|_, attempt| {
				if attempt == 1 {
					grammar_error_response()
				} else {
					success_response()
				}
			});
			let transcript = transcript(vec![tool("lookup", Some("prefer"))]);
			stream(&model(), &transcript, Some(options.clone())).result().await;
			stream(&model(), &transcript, Some(options)).result().await;
			assert_eq!(
				payloads
					.borrow()
					.iter()
					.map(|payload| tool_at(payload, 0).get("strict").cloned())
					.collect::<Vec<_>>(),
				[Some(json!(true)), None, Some(json!(true))]
			);
		});
	}
}
