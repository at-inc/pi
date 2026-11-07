//! Port of `packages/ai/test/pi-messages.test.ts`.
//!
//! The TS suite serves the backend from a `node:http` server; these tests serve the same responses through
//! `TestHost`'s fetch handler and assert the same requests and results.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::pi_messages::{PiMessagesOptions, stream, stream_simple};
use pi_ai::types::{
	AssistantMessage, BaseModel, Context, Model, ModelCost, ModelInputModality, ProviderHeaders,
	ProviderRequestOptions, SimpleStreamOptions, StopReason, StreamOptions, TranscriptContext, Usage,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::HttpResponse;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

const TIMESTAMP: u64 = 1_767_225_600_000;

fn model() -> Model {
	Model {
		base: BaseModel {
			id: "auto".into(),
			name: "Radius Auto".into(),
			api: "pi-messages".into(),
			provider: "radius".into(),
			base_url: "http://127.0.0.1:4321/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost {
				input: 1.0,
				output: 2.0,
				cache_read: 0.1,
				cache_write: 0.2,
				..ModelCost::default()
			},
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 128000.0,
		max_tokens: 16384.0,
		..Model::default()
	}
}

fn context_json() -> Value {
	json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": TIMESTAMP }] })
}

fn context() -> TranscriptContext {
	let context: Context = serde_json::from_value(context_json()).unwrap();
	normalize_context(&context)
}

fn usage() -> Value {
	json!({
		"input": 10,
		"output": 5,
		"cacheRead": 0,
		"cacheWrite": 0,
		"totalTokens": 15,
		"cost": { "input": 0.1, "output": 0.2, "cacheRead": 0, "cacheWrite": 0, "total": 0.3 },
	})
}

/// The TS `startServer`: a 200 SSE response with one `data:` frame per event.
fn serve_events(host: &TestHost, headers: &[(&str, &str)], events: &[Value]) {
	let body: String = events.iter().map(|event| format!("data: {event}\n\n")).collect();
	let mut response_headers = IndexMap::new();
	response_headers.insert("content-type".to_string(), "text/event-stream".to_string());
	for (name, value) in headers {
		response_headers.insert((*name).to_string(), (*value).to_string());
	}
	host.on_fetch(move |_| {
		let (headers, body) = (response_headers.clone(), body.clone());
		Box::pin(async move { Ok(HttpResponse::from_bytes(200, headers, body)) })
	});
}

fn options(options: StreamOptions) -> StreamOptions {
	StreamOptions {
		base: ProviderRequestOptions {
			api_key: Some("test-key".into()),
			..options.base
		},
		..options
	}
}

fn to_value(message: &AssistantMessage) -> Value {
	serde_json::to_value(message).unwrap()
}

async fn result(options: Option<PiMessagesOptions>) -> AssistantMessage {
	stream(&model(), &context(), options).result().await.unwrap()
}

mod pi_messages {
	use super::*;

	#[test]
	fn streams_text_and_tool_calls_and_resolves_the_terminal_message() {
		run(|host| async move {
			serve_events(
				&host,
				&[],
				&[
					json!({ "type": "start" }),
					json!({ "type": "text_start", "contentIndex": 0 }),
					json!({ "type": "text_delta", "contentIndex": 0, "delta": "Hel" }),
					json!({ "type": "text_delta", "contentIndex": 0, "delta": "lo" }),
					json!({ "type": "text_end", "contentIndex": 0, "content": "Hello" }),
					json!({ "type": "toolcall_start", "contentIndex": 1, "id": "call_1", "toolName": "read" }),
					json!({ "type": "toolcall_delta", "contentIndex": 1, "delta": "{\"path\":" }),
					json!({ "type": "toolcall_delta", "contentIndex": 1, "delta": "\"a.txt\"}" }),
					json!({
						"type": "toolcall_end",
						"contentIndex": 1,
						"toolCall": { "type": "toolCall", "id": "call_1", "name": "read", "arguments": { "path": "a.txt" } },
					}),
					json!({
						"type": "done",
						"reason": "toolUse",
						"usage": usage(),
						"responseId": "resp_1",
						"responseModel": "claude-opus-5-5",
						"providerThinkingLevel": "high",
					}),
				],
			);
			let mut headers = ProviderHeaders::new();
			headers.insert("x-custom".into(), Some("1".into()));
			let event_stream = stream(
				&model(),
				&context(),
				Some(PiMessagesOptions {
					base: options(StreamOptions {
						base: ProviderRequestOptions {
							headers: Some(headers),
							..ProviderRequestOptions::default()
						},
						session_id: Some("session-1".into()),
						max_tokens: Some(100.0),
						..StreamOptions::default()
					}),
					tool_choice: Some(json!("auto")),
					..PiMessagesOptions::default()
				}),
			);
			// The test future only runs once every spawned task waits, so the consumer is a spawned task like the producer,
			// which lets it read each event's partial before the next event arrives.
			let (sender, receiver) = futures::channel::oneshot::channel();
			let consumer = event_stream.clone();
			pi_host::spawn_local(async move {
				let mut events = Vec::new();
				let mut partial_stop_reasons = Vec::new();
				while let Some(event) = consumer.next().await {
					let event = serde_json::to_value(&event).unwrap();
					if let Some(partial) = event.get("partial") {
						partial_stop_reasons.push(partial["stopReason"].clone());
					}
					events.push(event);
				}
				let _ = sender.send((events, partial_stop_reasons));
			});
			let (events, partial_stop_reasons) = receiver.await.unwrap();
			let message = event_stream.result().await.unwrap();

			assert_eq!(partial_stop_reasons[0], json!("pending"));
			assert_eq!(message.stop_reason, StopReason::ToolUse);
			assert_eq!(message.usage, serde_json::from_value::<Usage>(usage()).unwrap());
			assert_eq!(message.response_id.as_deref(), Some("resp_1"));
			assert_eq!(message.provider_thinking_level.as_deref(), Some("high"));
			assert_eq!(message.model, "auto");
			assert_eq!(message.response_model.as_deref(), Some("claude-opus-5-5"));
			assert_eq!(message.provider, "radius");
			assert_eq!(
				to_value(&message)["content"],
				json!([
					{ "type": "text", "text": "Hello" },
					{ "type": "toolCall", "id": "call_1", "name": "read", "arguments": { "path": "a.txt" } },
				])
			);
			assert!(events.iter().any(|event| event["type"] == "text_delta"));
			assert_eq!(events.iter().filter(|event| event["type"] == "toolcall_end").count(), 1);

			let requests = host.requests();
			assert_eq!(requests.len(), 1);
			let request = &requests[0];
			assert_eq!(request.url, "http://127.0.0.1:4321/v1/messages");
			assert_eq!(request.header("authorization"), Some("Bearer test-key"));
			assert_eq!(request.header("x-custom"), Some("1"));
			assert_eq!(
				request.json(),
				json!({
					"model": "auto",
					"context": context_json(),
					"options": { "maxTokens": 100, "sessionId": "session-1", "toolChoice": "auto" },
				})
			);
		});
	}

	#[test]
	fn forwards_parsed_wire_events_in_order_before_converting_them() {
		run(|host| async move {
			let wire_events = vec![
				json!({ "type": "start" }),
				json!({ "type": "text_start", "contentIndex": 0 }),
				json!({ "type": "text_delta", "contentIndex": 0, "delta": "Hello", "gatewayField": "upstream-value" }),
				json!({ "type": "text_end", "contentIndex": 0, "content": "Hello" }),
				json!({ "type": "done", "reason": "stop", "usage": usage(), "responseId": "resp_1" }),
			];
			serve_events(&host, &[], &wire_events);
			let received = Rc::new(RefCell::new(Vec::new()));
			let event_models = Rc::new(RefCell::new(Vec::new()));
			let (received_events, received_models) = (received.clone(), event_models.clone());
			let message = stream_simple(
				&model(),
				&context(),
				Some(SimpleStreamOptions {
					base: options(StreamOptions {
						on_provider_stream_event: Some(Rc::new(move |event: &Value, event_model: &Model| {
							let (received, models) = (received_events.clone(), received_models.clone());
							let (event, event_model) = (event.clone(), event_model.clone());
							Box::pin(async move {
								futures::future::ready(()).await;
								received.borrow_mut().push(event);
								models.borrow_mut().push(event_model);
								Ok(())
							})
						})),
						..StreamOptions::default()
					}),
					..SimpleStreamOptions::default()
				}),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(*received.borrow(), wire_events);
			assert_eq!(*event_models.borrow(), vec![model(); wire_events.len()]);
			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(message.response_id.as_deref(), Some("resp_1"));
			assert_eq!(to_value(&message).get("responseModel"), None);
			assert_eq!(
				to_value(&message)["content"],
				json!([{ "type": "text", "text": "Hello" }])
			);
		});
	}

	// The TS passes `debug` to `streamSimple` as an extra `SimpleStreamOptions` key; the Rust `SimpleStreamOptions`
	// has no extra keys, so the port passes it to `stream`.
	#[test]
	fn appends_debug_1_and_reports_response_headers_via_on_response() {
		run(|host| async move {
			serve_events(
				&host,
				&[("x-pi-gateway-upstream-provider", "anthropic")],
				&[json!({ "type": "done", "reason": "stop", "usage": usage() })],
			);
			let observed_headers = Rc::new(RefCell::new(None));
			let observed = observed_headers.clone();
			let message = result(Some(PiMessagesOptions {
				base: options(StreamOptions {
					base: ProviderRequestOptions {
						on_response: Some(Rc::new(move |response, _| {
							*observed.borrow_mut() = Some(response.headers);
							Box::pin(async { Ok(()) })
						})),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				}),
				debug: Some(json!(true)),
				..PiMessagesOptions::default()
			}))
			.await;

			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(host.requests()[0].url, "http://127.0.0.1:4321/v1/messages?debug=1");
			let observed_headers = observed_headers.borrow();
			assert_eq!(
				observed_headers
					.as_ref()
					.unwrap()
					.get("x-pi-gateway-upstream-provider")
					.map(String::as_str),
				Some("anthropic")
			);
		});
	}

	#[test]
	fn surfaces_backend_error_responses_with_diagnostics() {
		run(|host| async move {
			host.on_fetch(|_| {
				Box::pin(async {
					let mut headers = IndexMap::new();
					headers.insert("content-type".to_string(), "application/json".to_string());
					let mut response = HttpResponse::from_bytes(
						401,
						headers,
						r#"{"error":{"message":"Token expired","code":"unauthorized"}}"#,
					);
					response.status_text = "Unauthorized".into();
					Ok(response)
				})
			});

			let message = result(Some(PiMessagesOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some("stale".into()),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				},
				..PiMessagesOptions::default()
			}))
			.await;

			assert_eq!(message.stop_reason, StopReason::Error);
			let error_message = message.error_message.as_deref().unwrap();
			assert_eq!(error_message, "401 Unauthorized: Token expired (unauthorized)");
			let diagnostics = message.diagnostics.as_ref().unwrap();
			assert_eq!(diagnostics[0].r#type, "pi_messages_response_failure");
			assert_eq!(diagnostics[0].details.as_ref().unwrap()["status"], json!(401));
		});
	}

	#[test]
	fn propagates_server_sent_error_events() {
		run(|host| async move {
			serve_events(
				&host,
				&[],
				&[
					json!({ "type": "start" }),
					json!({
						"type": "error",
						"reason": "error",
						"usage": usage(),
						"errorMessage": "Upstream failed",
						"responseModel": "claude-opus-5-5",
					}),
				],
			);

			let message = result(Some(PiMessagesOptions {
				base: options(StreamOptions::default()),
				..PiMessagesOptions::default()
			}))
			.await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.error_message.as_deref(), Some("Upstream failed"));
			assert_eq!(message.usage, serde_json::from_value::<Usage>(usage()).unwrap());
			assert_eq!(message.response_model.as_deref(), Some("claude-opus-5-5"));
		});
	}

	#[test]
	fn errors_when_no_api_key_is_provided() {
		run(|host| async move {
			let message = result(None).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(
				message.error_message.as_deref(),
				Some("No API key provided for provider \"radius\"")
			);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn rejects_content_indexes_that_are_not_the_next_or_an_existing_block() {
		let cases = [
			(
				vec![json!({ "type": "text_start", "contentIndex": 4294967000u64 })],
				"Invalid contentIndex 4294967000 in pi-messages text_start event",
			),
			(
				vec![json!({ "type": "text_start", "contentIndex": "__proto__" })],
				"Invalid contentIndex \"__proto__\" in pi-messages text_start event",
			),
			(
				vec![
					json!({ "type": "text_start", "contentIndex": 0 }),
					json!({ "type": "toolcall_end", "contentIndex": "__proto__", "toolCall": { "polluted": true } }),
				],
				"Invalid contentIndex \"__proto__\" in pi-messages toolcall_end event",
			),
			(
				vec![json!({ "type": "text_delta", "contentIndex": 0, "delta": "x" })],
				"Invalid contentIndex 0 in pi-messages text_delta event",
			),
		];
		for (events, expected) in cases {
			run(|host| async move {
				let mut wire_events = vec![json!({ "type": "start" })];
				wire_events.extend(events);
				wire_events.push(json!({ "type": "done", "reason": "stop", "usage": usage() }));
				serve_events(&host, &[], &wire_events);

				let message = result(Some(PiMessagesOptions {
					base: options(StreamOptions::default()),
					..PiMessagesOptions::default()
				}))
				.await;

				assert_eq!(message.stop_reason, StopReason::Error);
				assert_eq!(message.error_message.as_deref(), Some(expected));
				assert!(message.content.len() <= 1);
			});
		}
	}

	#[test]
	fn errors_when_the_stream_ends_without_a_terminal_event() {
		run(|host| async move {
			serve_events(
				&host,
				&[],
				&[
					json!({ "type": "start" }),
					json!({ "type": "text_start", "contentIndex": 0 }),
					json!({ "type": "text_delta", "contentIndex": 0, "delta": "partial" }),
				],
			);

			let message = result(Some(PiMessagesOptions {
				base: options(StreamOptions::default()),
				..PiMessagesOptions::default()
			}))
			.await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(
				message.error_message.as_deref(),
				Some("radius stream ended without a terminal event")
			);
		});
	}

	// Not in the TS suite: `fetch` gives a 204 response a null body, which hits the `!response.body` branch.
	#[test]
	fn reports_a_null_body_status_as_a_missing_body() {
		run(|host| async move {
			host.on_fetch(|_| Box::pin(async { Ok(HttpResponse::from_bytes(204, IndexMap::new(), "")) }));

			let message = result(Some(PiMessagesOptions {
				base: options(StreamOptions::default()),
				..PiMessagesOptions::default()
			}))
			.await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.error_message.as_deref(), Some("radius response has no body"));
		});
	}

	// Not in the TS suite: the TS only stops on abort when the body's read rejects, so a body that ignores the signal
	// is read to its terminal event.
	#[test]
	fn keeps_reading_a_body_that_ignores_the_abort() {
		run(|host| async move {
			host.on_fetch(|_| {
				Box::pin(async {
					let start =
						futures::stream::once(async { Ok(format!("data: {}\n\n", json!({ "type": "start" })).into()) });
					let done = futures::stream::once(async {
						pi_host::sleep(50.0, None).await?;
						let done =
							json!({ "type": "done", "reason": "stop", "usage": usage(), "responseId": "resp_1" });
						Ok(format!("data: {done}\n\n").into())
					});
					Ok(HttpResponse {
						status: 200,
						status_text: "OK".into(),
						url: String::new(),
						headers: IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
						body: futures::StreamExt::boxed_local(futures::StreamExt::chain(start, done)),
					})
				})
			});
			let controller = pi_host::AbortController::new();
			let abort = controller.clone();

			let message = result(Some(PiMessagesOptions {
				base: options(StreamOptions {
					base: ProviderRequestOptions {
						signal: Some(controller.signal()),
						..ProviderRequestOptions::default()
					},
					on_provider_stream_event: Some(Rc::new(move |event: &Value, _: &Model| {
						if event["type"] == "start" {
							abort.abort();
						}
						Box::pin(async { Ok(()) })
					})),
					..StreamOptions::default()
				}),
				..PiMessagesOptions::default()
			}))
			.await;

			assert!(controller.signal().aborted());
			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(message.response_id.as_deref(), Some("resp_1"));
		});
	}
}

// The TS case "is a known api usable on models" is a type-level check with no runtime behavior to port.
mod pi_messages_api_registration {
	use pi_ai::compat::get_api_provider;

	#[test]
	fn is_registered_as_a_builtin_api_provider() {
		pi_host::testing::run(|_| async {
			assert!(get_api_provider("pi-messages").is_some());
		});
	}
}
