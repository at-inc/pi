//! Port of `packages/ai/test/bedrock-strict-tool-fallback.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@aws-sdk/client-bedrock-runtime` and records a `structuredClone` of each `ConverseStreamCommand`
//!   input in `send`. The port records what the client sent: the JSON body of each request, which is the command input
//!   (after `onPayload`) without `modelId` (that goes into the URL path). Credentials come from
//!   `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` in the process env, which the mocked client did not need.
//! - `sdkError("ValidationException", message)` is a 400 response with `x-amzn-errortype: ValidationException` and a
//!   JSON `{ "message": ... }` body, which the client turns into the same service exception.
//! - `vi.fn` is a call counter. `toMatchObject` checks the listed fields one by one; `expect.objectContaining` checks
//!   the listed keys of the one diagnostic.

mod bedrock_support;

use std::cell::Cell;
use std::rc::Rc;

use bedrock_support::{Reply, body, collect, event, json_response, serve, stream_response};
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{AssistantMessage, CacheRetention, Model, OnPayload, StopReason, StreamOptions};
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

/// The `model` of the TS file.
fn model() -> Model {
	bedrock_support::model(json!({
		"id": "us.anthropic.claude-opus-5-5",
		"name": "Claude Opus 5.5 (US)",
		"api": "bedrock-converse-stream",
		"provider": "amazon-bedrock",
		"baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 200000,
		"maxTokens": 100,
		"compat": { "supportsStrictMode": true },
	}))
}

// Observed from live Bedrock with 20 optional parameters across strict tools.
const UNION_LIMIT_MESSAGE: &str = "The model returned the following errors: Schemas contains too many parameters with union types (20 parameters with type arrays or anyOf). This causes exponential compilation cost. Reduce the number of nullable or union-typed parameters (limit: 16 parameters with unions).";

/// `tool(name, strict)`: `Type.Object({ value: Type.String(), optional: Type.Optional(Type.Number()) })` parameters.
fn tool(name: &str, strict: Option<&str>) -> Value {
	let mut tool = json!({
		"name": name,
		"description": "Look up a value",
		"parameters": {
			"type": "object",
			"properties": { "value": { "type": "string" }, "optional": { "type": "number" } },
			"required": ["value"],
		},
	});
	if let Some(strict) = strict {
		tool["constrainedSampling"] = json!({ "type": "json_schema", "strict": strict });
	}
	tool
}

/// `sdkError(name, message)` as the response the client builds it from.
fn sdk_error(name: &str, message: &str) -> Reply {
	json_response(
		400,
		&[("x-amzn-errortype", name), ("content-type", "application/json")],
		&json!({ "message": message }).to_string(),
	)
}

/// `successResponse()`.
fn success_response() -> Reply {
	stream_response(
		&[
			event("messageStart", json!({ "role": "assistant" })),
			event("messageStop", json!({ "stopReason": "end_turn" })),
		],
		&[],
	)
}

/// Static AWS credentials in the process env, which the default chain picks up.
fn set_credentials(host: &TestHost) {
	host.set_env("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE");
	host.set_env("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
}

/// `run(tools, onPayload)`, and the inputs the client sent.
async fn run_bedrock(
	host: &TestHost,
	tools: Vec<Value>,
	on_payload: Option<OnPayload>,
) -> (AssistantMessage, Vec<Value>) {
	let mut options = BedrockOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..BedrockOptions::default()
	};
	options.on_payload = on_payload;
	let context = bedrock_support::context(
		json!({ "tools": tools, "messages": [{ "role": "user", "content": "Hello", "timestamp": 1 }] }),
	);
	let (_, result) = collect(stream(&model(), &context, Some(options))).await;
	(result, host.requests().iter().map(body).collect())
}

// Regression tests for https://github.com/earendil-works/pi/issues/11416.
mod bedrock_preferred_strict_tool_fallback {
	use super::*;

	#[test]
	fn retries_a_strict_tool_limit_rejection_with_preferred_tools_non_strict_and_keeps_required_tools_strict() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![
					sdk_error("ValidationException", UNION_LIMIT_MESSAGE),
					success_response(),
				],
			);
			let calls = Rc::new(Cell::new(0));
			let counter = calls.clone();
			let on_payload: OnPayload = Rc::new(move |payload: Value, _model: &Model| {
				counter.set(counter.get() + 1);
				let mut payload = payload;
				payload["requestMetadata"] = json!({ "team": "a" });
				Box::pin(async move { Ok(Some(payload)) })
			});

			let (result, inputs) = run_bedrock(
				&host,
				vec![
					tool("preferred", Some("prefer")),
					tool("required", Some("require")),
					tool("ordinary", None),
				],
				Some(on_payload),
			)
			.await;

			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(calls.get(), 2);
			let [first, second] = &inputs[..] else {
				panic!("expected two inputs, got {inputs:?}");
			};
			let first_spec = &first["toolConfig"]["tools"][0]["toolSpec"];
			assert_eq!(first_spec["strict"], json!(true));
			assert_eq!(
				first_spec["inputSchema"]["json"]["required"],
				json!(["value", "optional"])
			);
			assert_eq!(
				first_spec["inputSchema"]["json"]["properties"]["optional"],
				json!({ "anyOf": [{ "type": "number" }, { "type": "null" }] })
			);
			let second_spec = &second["toolConfig"]["tools"][0]["toolSpec"];
			assert_eq!(second_spec.get("strict"), None);
			assert_eq!(
				second_spec["inputSchema"]["json"],
				json!({
					"type": "object",
					"properties": { "value": { "type": "string" }, "optional": { "type": "number" } },
					"required": ["value"],
				})
			);
			assert_eq!(second["toolConfig"]["tools"][1], first["toolConfig"]["tools"][1]);
			assert_eq!(second["toolConfig"]["tools"][1]["toolSpec"]["strict"], json!(true));
			assert_eq!(second["toolConfig"]["tools"][2], first["toolConfig"]["tools"][2]);
			assert_eq!(second["requestMetadata"], json!({ "team": "a" }));
			let diagnostics = serde_json::to_value(&result.diagnostics).unwrap();
			let [diagnostic] = diagnostics.as_array().map(Vec::as_slice).unwrap_or_default() else {
				panic!("expected one diagnostic, got {diagnostics}");
			};
			assert_eq!(diagnostic["type"], json!("bedrock_strict_tools_fallback"));
			assert_eq!(diagnostic["error"], json!({ "message": UNION_LIMIT_MESSAGE }));
			assert_eq!(
				diagnostic["details"],
				json!({ "reason": "strict_tool_limits_exceeded" })
			);
		});
	}

	#[test]
	fn does_not_retry_without_preferred_strict_tools() {
		run(|host| async move {
			set_credentials(&host);
			serve(&host, vec![sdk_error("ValidationException", UNION_LIMIT_MESSAGE)]);

			let (result, inputs) = run_bedrock(
				&host,
				vec![tool("required", Some("require")), tool("ordinary", None)],
				None,
			)
			.await;

			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message,
				Some(format!("Validation error: {UNION_LIMIT_MESSAGE}"))
			);
			assert_eq!(inputs.len(), 1);
		});
	}

	#[test]
	fn does_not_retry_other_validation_errors() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![sdk_error("ValidationException", "messages: field required")],
			);

			let (result, inputs) = run_bedrock(&host, vec![tool("preferred", Some("prefer"))], None).await;

			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(inputs.len(), 1);
		});
	}

	#[test]
	fn retries_only_once() {
		run(|host| async move {
			set_credentials(&host);
			serve(&host, vec![sdk_error("ValidationException", UNION_LIMIT_MESSAGE)]);

			let (result, inputs) = run_bedrock(&host, vec![tool("preferred", Some("prefer"))], None).await;

			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(inputs.len(), 2);
		});
	}
}
