//! Port of `packages/ai/test/provider-error-diagnostics.test.ts`, the cases that call `createProviderErrorDiagnostic`
//! directly (top-level tests), the Anthropic rows of the `$name` tables (`anthropic_` tests), and the cases that
//! stream through the Completions, Responses and Codex APIs (module `streaming_providers`, whose `it.each(providers)`
//! lists hold "completions", "responses" and "codex"). The
//! `JSON.parse(JSON.stringify(...))` round trip is `serde_json::to_value` of the diagnostic. The `Headers` instance,
//! `undefined` values and the `1n` payload have no counterpart in a `PiError`, whose `data` is already JSON, so those
//! inputs become plain JSON objects and the serialization fallback is not reachable.
//! `createProviderErrorDiagnostic(null)` becomes a thrown JSON `null`.

use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream as stream_anthropic};
use pi_ai::api::{openai_codex_responses, openai_completions, openai_responses};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, Context, Message, Model, ProviderRequestOptions,
	StopReason, StreamOptions, TranscriptContext, Transport, UserMessage,
};
use pi_ai::utils::diagnostics::{
	AssistantMessageDiagnostic, DiagnosticErrorCode, append_assistant_message_diagnostic,
	create_provider_error_diagnostic, extract_diagnostic_error, format_thrown_value,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiError;
use pi_host::testing::run;
use pi_host::{AbortController, FetchFunction, HttpResponse};
use serde_json::{Value, json};

fn details(error: &PiError) -> Value {
	serde_json::to_value(diagnose(error).details).unwrap()
}

/// `createProviderErrorDiagnostic` stamps `Date.now()`, which reads the host clock.
fn diagnose(error: &PiError) -> AssistantMessageDiagnostic {
	pi_host::testing::run(|_| async move { create_provider_error_diagnostic(error) })
}

#[test]
fn accepts_status_code_and_plain_header_records() {
	let payload = json!({ "message": "Plan limit" });
	let error = PiError::new("").with_data(json!({
		"statusCode": 403,
		"headers": { "x-request-id": "request" },
		"payload": payload,
	}));
	assert_eq!(
		details(&error),
		json!({ "status": 403, "headers": { "x-request-id": "request" }, "payload": payload })
	);
	assert_eq!(
		pi_host::testing::run(|_| async { create_provider_error_diagnostic(&Value::Null) }).r#type,
		"provider_error"
	);
}

#[test]
fn normalizes_optional_provider_metadata_to_json_values() {
	let error = PiError::new("Plan limit").with_status(429).with_data(json!({
		"headers": { "retry-after": "60" },
		"payload": { "message": "Plan limit", "values": [null, "retry"] },
	}));
	assert_eq!(
		details(&error),
		json!({
			"status": 429,
			"headers": { "retry-after": "60" },
			"payload": { "message": "Plan limit", "values": [null, "retry"] },
		})
	);
	assert_eq!(details(&PiError::new("Disconnected")), json!({}));
}

#[test]
fn keeps_the_original_failure_and_http_metadata() {
	let error = PiError::new("Plan limit").with_status(429).with_data(json!({
		"headers": { "retry-after": "60" },
		"payload": { "unsupported": 1 },
	}));
	let diagnostic = diagnose(&error);
	assert_eq!(
		diagnostic.error.as_ref().map(|error| error.message.as_str()),
		Some("Plan limit")
	);
	assert_eq!(
		serde_json::to_value(&diagnostic.details).unwrap(),
		json!({ "status": 429, "headers": { "retry-after": "60" }, "payload": { "unsupported": 1 } })
	);
	assert!(serde_json::to_string(&diagnostic).is_ok());
}

#[test]
fn falls_back_to_the_error_object_for_the_payload_and_ignores_non_numeric_status() {
	let error = PiError::new("Plan limit").with_data(json!({
		"status": "429",
		"headers": "retry-after: 60",
		"payload": null,
		"error": { "code": "usage_limit_reached" },
	}));
	assert_eq!(details(&error), json!({ "payload": { "code": "usage_limit_reached" } }));
	let scalar_error = PiError::new("Plan limit").with_data(json!({ "error": "usage_limit_reached" }));
	assert_eq!(details(&scalar_error), json!({}));
}

#[test]
fn extracts_error_name_message_and_code() {
	let error = PiError::named("RateLimitError", "").with_data(json!({ "code": "usage_limit_reached" }));
	let info = extract_diagnostic_error(&error);
	assert_eq!(info.name.as_deref(), Some("RateLimitError"));
	assert_eq!(info.message, "RateLimitError");
	assert_eq!(info.stack, None);
	assert_eq!(
		info.code,
		Some(DiagnosticErrorCode::String("usage_limit_reached".into()))
	);
	let numeric = extract_diagnostic_error(&PiError::new("Boom").with_data(json!({ "code": 42 })));
	assert_eq!(numeric.code, Some(DiagnosticErrorCode::Number(42.0)));
	assert_eq!(
		extract_diagnostic_error(&PiError::new("Boom").with_data(json!({ "code": true }))).code,
		None
	);
	assert_eq!(format_thrown_value(&PiError::named("AbortError", "")), "AbortError");
	assert_eq!(format_thrown_value(&PiError::new("Boom")), "Boom");
}

#[test]
fn appends_and_serializes_diagnostics_in_ts_field_order() {
	let mut message = AssistantMessage {
		content: vec![],
		api: "openai-responses".into(),
		provider: "openai".into(),
		model: "m".into(),
		stop_reason: StopReason::Error,
		..AssistantMessage::default()
	};
	let diagnostic = diagnose(&PiError::new("Boom").with_status(500));
	append_assistant_message_diagnostic(&mut message, diagnostic.clone());
	append_assistant_message_diagnostic(&mut message, diagnostic.clone());
	assert_eq!(message.diagnostics.as_ref().map(Vec::len), Some(2));
	let serialized = serde_json::to_string(&diagnostic).unwrap();
	let keys: Vec<&str> = ["\"type\"", "\"timestamp\"", "\"error\"", "\"details\""].to_vec();
	let positions: Vec<usize> = keys.iter().map(|key| serialized.find(key).unwrap()).collect();
	assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{serialized}");
}

#[test]
fn reports_non_error_throws_as_thrown_values() {
	let info = extract_diagnostic_error(&json!("boom"));
	assert_eq!(info.name.as_deref(), Some("ThrownValue"));
	assert_eq!(info.message, "boom");
	assert_eq!(info.code, None);
	assert_eq!(format_thrown_value(&json!(42)), "42");
	assert_eq!(format_thrown_value(&json!(null)), "null");
	assert_eq!(format_thrown_value(&json!([1, null, "a", [2, 3]])), "1,,a,2,3");
	assert_eq!(format_thrown_value(&json!({ "code": 1 })), "[object Object]");
	let thrown = json!({ "status": 429, "headers": { "retry-after": "60" }, "error": { "message": "Plan limit" } });
	let diagnostic = pi_host::testing::run(|_| async { create_provider_error_diagnostic(&thrown) });
	assert_eq!(
		diagnostic.error.as_ref().and_then(|error| error.name.as_deref()),
		Some("ThrownValue")
	);
	assert_eq!(
		diagnostic.error.as_ref().map(|error| error.message.as_str()),
		Some("[object Object]")
	);
	assert_eq!(
		serde_json::to_value(&diagnostic.details).unwrap(),
		json!({ "status": 429, "headers": { "retry-after": "60" }, "payload": { "message": "Plan limit" } })
	);
}

const TOKEN: &str = "test.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoidGVzdCJ9fQ.test";

/// `model("anthropic-messages")`.
fn anthropic_model() -> Model {
	serde_json::from_value(json!({
		"id": "test-model",
		"name": "Test",
		"api": "anthropic-messages",
		"provider": "test",
		"baseUrl": "https://provider.invalid/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 10000,
		"maxTokens": 1000,
	}))
	.unwrap()
}

/// The `anthropic` row's `stream(options)` with `{ apiKey: token, transport: "sse", maxRetries: 0, fetch }`.
async fn stream_anthropic_with(fetch: FetchFunction, signal: Option<pi_host::AbortSignal>) -> AssistantMessage {
	let context = normalize_context(
		&serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "Hello", "timestamp": 0 }] }))
			.unwrap(),
	);
	let options = AnthropicOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some(TOKEN.into()),
				max_retries: Some(0.0),
				signal,
				fetch: Some(fetch),
				..ProviderRequestOptions::default()
			},
			transport: Some(Transport::Sse),
			..StreamOptions::default()
		},
		..AnthropicOptions::default()
	};
	stream_anthropic(&anthropic_model(), &context, Some(options))
		.result()
		.await
		.unwrap()
}

#[test]
fn anthropic_preserves_http_metadata_through_json_serialization() {
	pi_host::testing::run(|_| async {
		let error = json!({
			"code": "usage_limit_reached",
			"type": "rate_limit_error",
			"message": "Plan limit",
			"plan_type": "pro",
			"resets_at": 2000000000,
		});
		let body = pi_host::js::json_stringify(&json!({ "error": error }));
		let fetch: FetchFunction = Rc::new(move |_| {
			let body = body.clone();
			Box::pin(async move {
				Ok(HttpResponse::from_bytes(
					429,
					IndexMap::from([
						("content-type".to_string(), "application/json".to_string()),
						("retry-after".to_string(), "60".to_string()),
						("x-request-id".to_string(), "test-request".to_string()),
					]),
					body,
				))
			})
		});
		let output = stream_anthropic_with(fetch, None).await;
		assert_eq!(output.stop_reason, StopReason::Error);
		let diagnostic = output
			.diagnostics
			.as_ref()
			.and_then(|diagnostics| diagnostics.iter().find(|item| item.r#type == "provider_error"))
			.map(|diagnostic| serde_json::to_value(diagnostic).unwrap())
			.unwrap_or(Value::Null);
		let details = &diagnostic["details"];
		assert_eq!(details["status"], json!(429));
		assert_eq!(details["headers"]["retry-after"], json!("60"));
		assert_eq!(details["headers"]["x-request-id"], json!("test-request"));
		// SDKs differ on whether .error includes the outer error envelope.
		let payload = &details["payload"];
		assert_eq!(
			payload.get("error").filter(|error| !error.is_null()).unwrap_or(payload),
			&error
		);
	});
}

#[test]
fn anthropic_does_not_classify_cancellation_as_a_provider_failure() {
	pi_host::testing::run(|_| async {
		let controller = AbortController::new();
		controller.abort();
		let fetch: FetchFunction = Rc::new(|_| Box::pin(async { Err(PiError::new("Request was aborted")) }));
		let output = stream_anthropic_with(fetch, Some(controller.signal())).await;
		assert_eq!(output.stop_reason, StopReason::Aborted);
		assert!(
			!output
				.diagnostics
				.as_ref()
				.is_some_and(|diagnostics| diagnostics.iter().any(|item| item.r#type == "provider_error"))
		);
	});
}

mod streaming_providers {
	use super::*;

	// `test.${base64url(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "test" } }))}.test`
	const TOKEN: &str = "test.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoidGVzdCJ9fQ.test";

	fn context() -> TranscriptContext {
		normalize_context(&Context {
			messages: vec![Message::User(
				serde_json::from_value::<UserMessage>(json!({ "role": "user", "content": "Hello", "timestamp": 0 }))
					.expect("user message"),
			)],
			..Context::default()
		})
	}

	fn model(api: &str) -> Model {
		serde_json::from_value(json!({
			"id": "test-model",
			"name": "Test",
			"api": api,
			"provider": "test",
			"baseUrl": "https://provider.invalid/v1",
			"reasoning": false,
			"input": ["text"],
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
			"contextWindow": 10000,
			"maxTokens": 1000,
		}))
		.expect("model JSON")
	}

	/// The `providers` list without "anthropic": `stream(options)` for each API.
	fn stream_with(name: &str, options: StreamOptions) -> AssistantMessageEventStream {
		match name {
			"completions" => openai_completions::stream(
				&model("openai-completions"),
				&context(),
				Some(openai_completions::OpenAICompletionsOptions {
					base: options,
					..Default::default()
				}),
			),
			"responses" => openai_responses::stream(
				&model("openai-responses"),
				&context(),
				Some(openai_responses::OpenAIResponsesOptions {
					base: options,
					..Default::default()
				}),
			),
			"codex" => openai_codex_responses::stream(
				&model("openai-codex-responses"),
				&context(),
				Some(openai_codex_responses::OpenAICodexResponsesOptions {
					base: options,
					..Default::default()
				}),
			),
			_ => unreachable!("unknown provider {name}"),
		}
	}

	const PROVIDERS: [&str; 3] = ["completions", "responses", "codex"];

	fn response(status: u16, headers: &[(&str, &str)], body: String) -> HttpResponse {
		let headers: IndexMap<String, String> = headers
			.iter()
			.map(|(name, value)| ((*name).to_string(), (*value).to_string()))
			.collect();
		HttpResponse::from_bytes(status, headers, body)
	}

	fn options(fetch: FetchFunction, configure: impl FnOnce(&mut StreamOptions)) -> StreamOptions {
		let mut options = StreamOptions {
			transport: Some(Transport::Sse),
			..StreamOptions::default()
		};
		options.api_key = Some(TOKEN.into());
		options.fetch = Some(fetch);
		configure(&mut options);
		options
	}

	fn diagnostics_json(message: &AssistantMessage) -> Vec<Value> {
		message
			.diagnostics
			.iter()
			.flatten()
			.map(|diagnostic| serde_json::to_value(diagnostic).expect("diagnostic serializes"))
			.collect()
	}

	#[test]
	fn name_preserves_http_metadata_through_json_serialization() {
		for name in PROVIDERS {
			run(|_| async move {
				let error = json!({
					"code": "usage_limit_reached",
					"type": "rate_limit_error",
					"message": "Plan limit",
					"plan_type": "pro",
					"resets_at": 2000000000,
				});
				let body = pi_host::js::json_stringify(&json!({ "error": error }));
				let fetch: FetchFunction = Rc::new(move |_| {
					let response = response(
						429,
						&[
							("content-type", "application/json"),
							("retry-after", "60"),
							("x-request-id", "test-request"),
						],
						body.clone(),
					);
					Box::pin(async move { Ok(response) })
				});
				let output = stream_with(name, options(fetch, |options| options.max_retries = Some(0.0)))
					.result()
					.await
					.expect("stream has a result");

				assert_eq!(output.stop_reason, StopReason::Error, "{name}");
				let diagnostic = diagnostics_json(&output)
					.into_iter()
					.find(|item| item["type"] == "provider_error")
					.unwrap_or_else(|| panic!("{name}: provider_error diagnostic"));
				let details = &diagnostic["details"];
				assert_eq!(details["status"].as_f64(), Some(429.0), "{name}");
				assert_eq!(details["headers"]["retry-after"], "60", "{name}");
				assert_eq!(details["headers"]["x-request-id"], "test-request", "{name}");
				// SDKs differ on whether .error includes the outer error envelope.
				let payload = details["payload"].get("error").unwrap_or(&details["payload"]);
				assert_eq!(payload, &error, "{name}");
			});
		}
	}

	#[test]
	fn name_does_not_classify_cancellation_as_a_provider_failure() {
		for name in PROVIDERS {
			run(|_| async move {
				let controller = AbortController::new();
				controller.abort();
				let fetch: FetchFunction = Rc::new(|_| Box::pin(async { Err(PiError::new("Request was aborted")) }));
				let output = stream_with(
					name,
					options(fetch, |options| {
						options.max_retries = Some(0.0);
						options.signal = Some(controller.signal());
					}),
				)
				.result()
				.await
				.expect("stream has a result");

				assert_eq!(output.stop_reason, StopReason::Aborted, "{name}");
				assert!(
					!diagnostics_json(&output)
						.iter()
						.any(|item| item["type"] == "provider_error"),
					"{name}"
				);
			});
		}
	}

	#[test]
	fn preserves_codex_stream_error_events() {
		run(|_| async {
			let event = json!({
				"type": "error",
				"code": "usage_limit_reached",
				"message": "Plan limit",
				"plan_type": "pro",
				"resets_at": 2000000000,
			});
			let body = format!("data: {}\n\n", pi_host::js::json_stringify(&event));
			let fetch: FetchFunction = Rc::new(move |_| {
				let response = response(200, &[("content-type", "text/event-stream")], body.clone());
				Box::pin(async move { Ok(response) })
			});
			let output = stream_with("codex", options(fetch, |_| {}))
				.result()
				.await
				.expect("stream has a result");

			assert!(
				diagnostics_json(&output).iter().any(|item| {
					item["type"] == "provider_error"
						&& item["error"]["code"] == "usage_limit_reached"
						&& item["details"]["payload"] == event
				}),
				"{:?}",
				diagnostics_json(&output)
			);
		});
	}

	#[test]
	fn retains_identity_before_output_item_done() {
		for phase in ["commentary", "final_answer"] {
			run(|_| async move {
				let events = [
					json!({
						"type": "response.output_item.added",
						"output_index": 0,
						"item": { "type": "message", "id": "msg_test", "role": "assistant", "status": "in_progress", "phase": phase, "content": [] },
					}),
					json!({
						"type": "response.output_text.delta",
						"output_index": 0,
						"content_index": 0,
						"item_id": "msg_test",
						"delta": "Partial answer",
					}),
				];
				let body: String = events
					.iter()
					.map(|event| format!("data: {}\n\n", pi_host::js::json_stringify(event)))
					.collect();
				let fetch: FetchFunction = Rc::new(move |_| {
					let response = response(200, &[("content-type", "text/event-stream")], body.clone());
					Box::pin(async move { Ok(response) })
				});
				let mut stream_options = options(fetch, |_| {});
				stream_options.api_key = Some("test".into());
				stream_options.transport = None;
				let stream = stream_with("responses", stream_options);
				let mut starting_signature = None;
				while let Some(event) = stream.next().await {
					if let AssistantMessageEvent::TextStart { content_index, partial } = &event
						&& let Some(AssistantContent::Text(block)) = partial.borrow().content.get(*content_index)
					{
						starting_signature = block.text_signature.clone();
					}
				}
				let output = stream.result().await.expect("stream has a result");

				assert_eq!(output.stop_reason, StopReason::Error, "{phase}");
				let expected = pi_host::js::json_stringify(&json!({ "v": 1, "id": "msg_test", "phase": phase }));
				assert_eq!(starting_signature.as_deref(), Some(expected.as_str()), "{phase}");
				assert_eq!(
					serde_json::to_value(&output.content).unwrap(),
					json!([{ "type": "text", "text": "Partial answer", "textSignature": expected }]),
					"{phase}"
				);
			});
		}
	}
}
