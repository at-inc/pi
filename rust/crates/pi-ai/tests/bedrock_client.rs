//! Rust-only: the Bedrock Runtime client behaviour that the TS gets from `@aws-sdk/client-bedrock-runtime`, end to end
//! through `bedrock_converse_stream::stream`. The expected values were recorded by running the TS adapter with the real
//! SDK against a local server (requests, events, provider events and the final message).
//!
//! The recorded requests also carry `host` and `Connection`, which the host's fetch adds on its own, and the
//! Node user agent sections (`os/linux#<release> lang/js md/nodejs#<version>`).

mod bedrock_support;

use std::cell::RefCell;
use std::rc::Rc;

use bedrock_support::{
	builtin_model, collect, error_message, event, exception, frame, json_response, serve, stream_response,
};
use indexmap::IndexMap;
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{
	AssistantMessage, CacheRetention, Model, ProviderRequestOptions, StopReason, StreamOptions, TranscriptContext,
};
use pi_ai::utils::identity::get_client_version;
use pi_host::AbortController;
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::{Value, json};

const HINT: &str =
	"\n  Deserialization error: to see the raw response, inspect the hidden field {error}.$response on this object.";

fn opus(base_url: &str) -> Model {
	let mut model = builtin_model("us.anthropic.claude-opus-4-8");
	model.base_url = base_url.into();
	model
}

fn hello() -> TranscriptContext {
	bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }))
}

/// `{ apiKey: "tok", cacheRetention: "none" }`.
fn bearer_options() -> BedrockOptions {
	BedrockOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("tok".into()),
				..ProviderRequestOptions::default()
			},
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..BedrockOptions::default()
	}
}

/// Name, status, headers, body, attempts, error message and diagnostic details.
type HttpErrorCase = (
	&'static str,
	u16,
	Vec<(&'static str, &'static str)>,
	String,
	usize,
	String,
	Value,
);

/// Name, messages, headers, events, error message and diagnostics.
type StreamErrorCase = (
	&'static str,
	Vec<Vec<u8>>,
	Vec<(&'static str, &'static str)>,
	Vec<&'static str>,
	String,
	Value,
);

struct Outcome {
	requests: Vec<RecordedRequest>,
	events: Vec<&'static str>,
	provider_events: Vec<Value>,
	message: AssistantMessage,
}

async fn run_case(host: &TestHost, model: &Model, context: &TranscriptContext, mut options: BedrockOptions) -> Outcome {
	let provider_events = Rc::new(RefCell::new(Vec::new()));
	let recorded = provider_events.clone();
	options.on_provider_stream_event = Some(Rc::new(move |item: &Value, _model: &Model| {
		recorded.borrow_mut().push(item.clone());
		Box::pin(async { Ok(()) })
	}));
	let (events, message) = collect(stream(model, context, Some(options))).await;
	Outcome {
		requests: host.requests(),
		events,
		provider_events: provider_events.take(),
		message,
	}
}

/// The `details` of each diagnostic (the timestamps differ).
fn diagnostic_details(message: &AssistantMessage) -> Value {
	let value = serde_json::to_value(message).unwrap();
	match value.get("diagnostics") {
		Some(Value::Array(diagnostics)) => Value::Array(
			diagnostics
				.iter()
				.map(|diagnostic| json!({ "type": diagnostic["type"], "details": diagnostic["details"] }))
				.collect(),
		),
		_ => Value::Null,
	}
}

fn failure(details: Value) -> Value {
	json!([{ "type": "bedrock_response_failure", "details": details }])
}

fn assert_error(outcome: &Outcome, events: &[&str], message: &str, diagnostics: Value) {
	assert_eq!(outcome.events, events);
	assert_eq!(outcome.message.stop_reason, StopReason::Error);
	assert_eq!(outcome.message.error_message.as_deref(), Some(message));
	assert_eq!(diagnostic_details(&outcome.message), diagnostics);
}

#[test]
fn signs_a_sigv4_request_like_the_sdk() {
	run(|host| async move {
		serve(
			&host,
			vec![stream_response(
				&[
					event("messageStart", json!({ "role": "assistant", "p": "abc" })),
					event(
						"contentBlockDelta",
						json!({ "contentBlockIndex": 0, "delta": { "text": "hi" }, "p": "x" }),
					),
					event("contentBlockStop", json!({ "contentBlockIndex": 0 })),
					event(
						"messageStop",
						json!({ "stopReason": "end_turn", "additionalModelResponseFields": { "a": 1 } }),
					),
					event(
						"metadata",
						json!({ "usage": { "inputTokens": 3, "outputTokens": 2, "totalTokens": 5 }, "metrics": { "latencyMs": 10 } }),
					),
				],
				&[],
			)],
		);
		let context = bedrock_support::context(json!({
			"systemPrompt": "Be brief.",
			"messages": [{ "role": "user", "content": "hello", "timestamp": 1 }],
		}));
		let options = BedrockOptions {
			base: StreamOptions {
				base: ProviderRequestOptions {
					env: Some(IndexMap::from([
						("AWS_ACCESS_KEY_ID".to_string(), "AKIDEXAMPLE".to_string()),
						(
							"AWS_SECRET_ACCESS_KEY".to_string(),
							"wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
						),
						("AWS_REGION".to_string(), "us-west-2".to_string()),
					])),
					headers: Some(IndexMap::from([
						("x-custom".to_string(), Some("v".to_string())),
						("X-Amz-Date".to_string(), Some("evil".to_string())),
					])),
					..ProviderRequestOptions::default()
				},
				temperature: Some(0.5),
				..StreamOptions::default()
			},
			..BedrockOptions::default()
		};
		let outcome = run_case(&host, &opus("http://127.0.0.1:45123"), &context, options).await;

		assert_eq!(
			outcome.events,
			["start", "text_start", "text_delta", "text_end", "done"]
		);
		assert_eq!(outcome.message.stop_reason, StopReason::Stop);
		assert_eq!(
			serde_json::to_value(&outcome.message.content).unwrap(),
			json!([{ "type": "text", "text": "hi" }])
		);
		assert_eq!(
			outcome.provider_events,
			[
				json!({ "messageStart": { "role": "assistant" } }),
				json!({ "contentBlockDelta": { "delta": { "text": "hi" }, "contentBlockIndex": 0 } }),
				json!({ "contentBlockStop": { "contentBlockIndex": 0 } }),
				json!({ "messageStop": { "stopReason": "end_turn", "additionalModelResponseFields": { "a": 1 } } }),
				json!({ "metadata": { "usage": { "inputTokens": 3, "outputTokens": 2, "totalTokens": 5 }, "metrics": { "latencyMs": 10 } } }),
			]
		);
		let [request] = &outcome.requests[..] else {
			panic!("expected one request, got {:?}", outcome.requests);
		};
		assert_eq!(
			request.url,
			"http://127.0.0.1:45123/model/us.anthropic.claude-opus-4-8/converse-stream"
		);
		assert_eq!(
			request.body.as_deref(),
			Some(
				r#"{"messages":[{"role":"user","content":[{"text":"hello"},{"cachePoint":{"type":"default"}}]}],"system":[{"text":"Be brief."},{"cachePoint":{"type":"default"}}],"inferenceConfig":{"maxTokens":128000,"temperature":0.5}}"#
			)
		);
		let names: Vec<&str> = request.headers.keys().map(String::as_str).collect();
		assert_eq!(
			names,
			[
				"content-type",
				"content-length",
				"x-amz-user-agent",
				"user-agent",
				"x-custom",
				"amz-sdk-invocation-id",
				"amz-sdk-request",
				"x-amz-date",
				"x-amz-content-sha256",
				"authorization",
			]
		);
		let header = |name: &str| request.headers.get(name).map(String::as_str).unwrap_or_default();
		let version = get_client_version(None);
		assert_eq!(header("content-length"), "216");
		assert_eq!(header("x-amz-user-agent"), format!("aws-sdk-js/3.1126.0 pi/{version}"));
		assert!(
			header("user-agent").starts_with("aws-sdk-js/3.1126.0 ua/2.1 os/"),
			"{}",
			header("user-agent")
		);
		assert!(
			header("user-agent").ends_with(&format!("api/bedrock-runtime#3.1126.0 m/N,E,e pi/{version}")),
			"{}",
			header("user-agent")
		);
		assert_eq!(header("x-custom"), "v");
		assert_eq!(header("amz-sdk-request"), "attempt=1; max=3");
		assert_eq!(
			header("x-amz-content-sha256"),
			"2ceba15d1c5fc3132e71ba2fe80537e22d509e7c446f91468e3ea925e64528ba"
		);
		assert_ne!(header("x-amz-date"), "evil");
		let date = &header("x-amz-date")[..8];
		assert!(
			header("authorization").starts_with(&format!(
				"AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/{date}/us-west-2/bedrock/aws4_request, SignedHeaders=amz-sdk-invocation-id;amz-sdk-request;content-length;content-type;host;x-amz-content-sha256;x-amz-date;x-amz-user-agent;x-custom, Signature="
			)),
			"{}",
			header("authorization")
		);
	});
}

#[test]
fn reports_a_bearer_validation_error_with_its_metadata() {
	run(|host| async move {
		serve(
			&host,
			vec![json_response(
				400,
				&[
					("x-amzn-errortype", "ValidationException:http://internal"),
					("x-amzn-requestid", "rid"),
					("content-type", "application/json"),
				],
				r#"{"message":"bad thing"}"#,
			)],
		);
		let outcome = run_case(&host, &opus("http://127.0.0.1:41319"), &hello(), bearer_options()).await;
		assert_error(
			&outcome,
			&["error"],
			"Validation error: bad thing",
			failure(json!({ "status": 400, "errorCode": "ValidationException", "requestId": "rid" })),
		);
		let [request] = &outcome.requests[..] else {
			panic!("expected one request");
		};
		let names: Vec<&str> = request.headers.keys().map(String::as_str).collect();
		assert_eq!(
			names,
			[
				"content-type",
				"content-length",
				"x-amz-user-agent",
				"user-agent",
				"amz-sdk-invocation-id",
				"amz-sdk-request",
				"Authorization",
			]
		);
		assert_eq!(request.headers["Authorization"], "Bearer tok");
		assert!(request.headers["user-agent"].contains(" m/N,E pi/"));
		assert_eq!(
			request.body.as_deref(),
			Some(
				r#"{"messages":[{"role":"user","content":[{"text":"hello"}]}],"inferenceConfig":{"maxTokens":128000}}"#
			)
		);
	});
}

#[test]
fn reports_http_errors_like_the_sdk() {
	let long_html = format!("<html>{}</html>", "x".repeat(300));
	let cases: Vec<HttpErrorCase> = vec![
		(
			"gateway-html",
			403,
			vec![("content-type", "text/html")],
			"<html>denied</html>".into(),
			1,
			format!("Unexpected token '<', \"<html>denied</html>\" is not valid JSON{HINT}"),
			json!({ "status": 403 }),
		),
		(
			"long-html",
			403,
			vec![("content-type", "text/html"), ("x-amzn-requestid", "rq")],
			long_html,
			1,
			format!("Unexpected token '<', \"<html>xxxx\"... is not valid JSON{HINT}"),
			json!({ "status": 403, "requestId": "rq" }),
		),
		(
			"empty-500",
			500,
			vec![],
			String::new(),
			3,
			"Unknown: UnknownError".into(),
			json!({ "status": 500 }),
		),
		(
			"Message-only",
			400,
			vec![("x-amzn-errortype", "AccessDeniedException")],
			r#"{"Message":"nope","extra":2}"#.into(),
			1,
			"AccessDeniedException: nope".into(),
			json!({ "status": 400, "errorCode": "AccessDeniedException" }),
		),
		(
			"unknown-code",
			418,
			vec![],
			r#"{"__type":"com.x#WeirdError","message":"odd"}"#.into(),
			1,
			"WeirdError: odd".into(),
			json!({ "status": 418 }),
		),
		(
			"throttle-429",
			429,
			vec![("x-amzn-errortype", "ThrottlingException")],
			r#"{"message":"slow"}"#.into(),
			3,
			"Throttling error: slow".into(),
			json!({ "status": 429, "errorCode": "ThrottlingException" }),
		),
	];
	for (name, status, headers, body, attempts, message, details) in cases {
		run(|host| async move {
			serve(&host, vec![json_response(status, &headers, &body)]);
			let outcome = run_case(&host, &opus("http://127.0.0.1:1"), &hello(), bearer_options()).await;
			assert_eq!(outcome.requests.len(), attempts, "{name}");
			assert_error(&outcome, &["error"], &message, failure(details));
		});
	}
}

#[test]
fn surfaces_stream_errors_like_the_sdk() {
	let error_frame = error_message("ModelStreamErrorException", "boom");
	let message_start = event("messageStart", json!({ "role": "assistant" }));
	let mut bad_crc = message_start.clone();
	*bad_crc.last_mut().unwrap() ^= 1;
	let mut bad_prelude = message_start.clone();
	bad_prelude[9] ^= 1;
	let cases: Vec<StreamErrorCase> = vec![
		(
			"exception-frame",
			vec![
				message_start.clone(),
				exception("throttlingException", json!({ "message": "slow down", "extra": 1 })),
			],
			vec![("x-amzn-requestid", "rid2")],
			vec!["start", "error"],
			"Throttling error: slow down".into(),
			failure(json!({ "errorCode": "ThrottlingException", "requestId": "rid2" })),
		),
		(
			"error-frame",
			vec![message_start.clone(), error_frame.clone()],
			vec![],
			vec!["start", "error"],
			"boom".into(),
			failure(json!({ "errorCode": "ModelStreamErrorException" })),
		),
		(
			"exception-unknown",
			vec![
				message_start.clone(),
				exception("weirdException", json!({ "message": "w" })),
			],
			vec![],
			vec!["start", "error"],
			r#"{"message":"w"}"#.into(),
			failure(json!({ "errorCode": "weirdException" })),
		),
		(
			"exception-noMessage",
			vec![
				message_start.clone(),
				exception("modelStreamErrorException", json!({ "originalStatusCode": 500 })),
			],
			vec![],
			vec!["start", "error"],
			"Model stream error: Unknown".into(),
			failure(json!({ "errorCode": "ModelStreamErrorException" })),
		),
		(
			"first-validation",
			vec![exception("validationException", json!({ "message": "bad" }))],
			vec![("x-amzn-requestid", "r1")],
			vec!["error"],
			"Validation error: bad".into(),
			failure(json!({ "errorCode": "ValidationException" })),
		),
		(
			"first-error-frame",
			vec![error_frame],
			vec![],
			vec!["error"],
			format!("boom{HINT}"),
			failure(json!({ "status": 200, "errorCode": "ModelStreamErrorException" })),
		),
		(
			"truncated",
			vec![
				message_start.clone(),
				event("messageStop", json!({ "stopReason": "end_turn" }))[..20].to_vec(),
			],
			vec![],
			vec!["start", "error"],
			"Truncated event message received.".into(),
			Value::Null,
		),
		(
			"truncated-first",
			vec![message_start[..20].to_vec()],
			vec![],
			vec!["error"],
			format!("Truncated event message received.{HINT}"),
			failure(json!({ "status": 200 })),
		),
		(
			"bad-crc",
			vec![bad_crc],
			vec![],
			vec!["error"],
			format!("The message checksum (482786840) did not match the expected value of 482786841{HINT}"),
			failure(json!({ "status": 200 })),
		),
		(
			"bad-prelude",
			vec![bad_prelude],
			vec![],
			vec!["error"],
			format!(
				"The prelude checksum specified in the message (2530540254) does not match the calculated CRC32 checksum (2530605790){HINT}"
			),
			failure(json!({ "status": 200 })),
		),
		(
			"bad-json-event",
			vec![frame("event", "messageStart", b"{nope", &[])],
			vec![],
			vec!["error"],
			format!("Expected property name or '}}' in JSON at position 1 (line 1 column 2){HINT}"),
			failure(json!({ "status": 200 })),
		),
	];
	for (name, messages, headers, events, message, diagnostics) in cases {
		run(|host| async move {
			serve(&host, vec![stream_response(&messages, &headers)]);
			let outcome = run_case(&host, &opus("http://127.0.0.1:1"), &hello(), bearer_options()).await;
			assert_eq!(outcome.requests.len(), 1, "{name}");
			assert_error(&outcome, &events, &message, diagnostics);
		});
	}
}

#[test]
fn retries_a_throttling_exception_in_the_first_frame() {
	run(|host| async move {
		serve(
			&host,
			vec![
				stream_response(
					&[exception("throttlingException", json!({ "message": "slow" }))],
					&[("x-amzn-requestid", "r1")],
				),
				stream_response(
					&[
						event("messageStart", json!({ "role": "assistant" })),
						event("messageStop", json!({ "stopReason": "end_turn" })),
					],
					&[],
				),
			],
		);
		let outcome = run_case(&host, &opus("http://127.0.0.1:1"), &hello(), bearer_options()).await;
		assert_eq!(outcome.requests.len(), 2);
		assert_eq!(outcome.requests[1].headers["amz-sdk-request"], "attempt=2; max=3");
		assert_eq!(
			outcome.requests[0].headers["amz-sdk-invocation-id"],
			outcome.requests[1].headers["amz-sdk-invocation-id"]
		);
		assert_eq!(outcome.events, ["start", "done"]);
		assert_eq!(outcome.message.stop_reason, StopReason::Stop);
	});
}

#[test]
fn sends_nothing_when_the_signal_is_already_aborted() {
	run(|host| async move {
		let controller = AbortController::new();
		controller.abort();
		let mut options = bearer_options();
		options.signal = Some(controller.signal());
		let outcome = run_case(&host, &opus("http://127.0.0.1:1"), &hello(), options).await;
		assert!(outcome.requests.is_empty());
		assert_eq!(outcome.events, ["error"]);
		assert_eq!(outcome.message.stop_reason, StopReason::Aborted);
		assert_eq!(outcome.message.error_message.as_deref(), Some("Request aborted"));
		assert_eq!(diagnostic_details(&outcome.message), Value::Null);
	});
}

#[test]
fn skips_unknown_events_and_keeps_unknown_union_members() {
	run(|host| async move {
		serve(
			&host,
			vec![stream_response(
				&[
					event("messageStart", json!({ "role": "assistant" })),
					event("futureEvent", json!({ "a": 1 })),
					event(
						"contentBlockDelta",
						json!({ "contentBlockIndex": 0, "delta": { "newThing": { "x": 1 } } }),
					),
					event(
						"contentBlockDelta",
						json!({ "contentBlockIndex": 0, "delta": { "reasoningContent": { "redactedContent": "AQID" } } }),
					),
					event("messageStop", json!({ "stopReason": "end_turn" })),
				],
				&[],
			)],
		);
		let outcome = run_case(&host, &opus("http://127.0.0.1:1"), &hello(), bearer_options()).await;
		assert_eq!(outcome.events, ["start", "thinking_start", "thinking_delta", "done"]);
		assert_eq!(
			outcome.provider_events,
			[
				json!({ "messageStart": { "role": "assistant" } }),
				json!({ "contentBlockDelta": { "delta": { "$unknown": ["newThing", { "x": 1 }] }, "contentBlockIndex": 0 } }),
				json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "redactedContent": "AQID" } }, "contentBlockIndex": 0 } }),
				json!({ "messageStop": { "stopReason": "end_turn" } }),
			]
		);
		assert_eq!(
			serde_json::to_value(&outcome.message.content).unwrap(),
			json!([{ "type": "thinking", "thinking": "[Reasoning redacted]", "thinkingSignature": "AQID", "redacted": true }])
		);
	});
}
