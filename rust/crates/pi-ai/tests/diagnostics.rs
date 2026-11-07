//! Port of `packages/ai/test/provider-error-diagnostics.test.ts`, the cases that call `createProviderErrorDiagnostic`
//! directly, and the Anthropic rows of the `$name` tables that stream through each API (`anthropic_` tests). The rows
//! for the OpenAI Completions, Responses and Codex APIs, `preserves Codex stream error events` and `retains %s identity
//! before output_item.done` move with those providers' w3 shards. The `JSON.parse(JSON.stringify(...))` round trip is
//! `serde_json::to_value` of the diagnostic. The `Headers` instance, `undefined` values and the `1n` payload have no
//! counterpart in a `PiError`, whose `data` is already JSON, so those inputs become plain JSON objects and the
//! serialization fallback is not reachable.
//! `createProviderErrorDiagnostic(null)` becomes a thrown JSON `null`.

use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::anthropic_messages::{AnthropicOptions, stream as stream_anthropic};
use pi_ai::types::{AssistantMessage, Model, ProviderRequestOptions, StopReason, StreamOptions, Transport};
use pi_ai::utils::diagnostics::{
	AssistantMessageDiagnostic, DiagnosticErrorCode, append_assistant_message_diagnostic,
	create_provider_error_diagnostic, extract_diagnostic_error, format_thrown_value,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiError;
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
