//! Port of `packages/ai/test/provider-error-body-regression.test.ts`.
//!
//! `vi.mock("openai")` threw an `APIError(403)` carrying the parsed body; here the test host answers the request with
//! that 403 and body, and the real client builds the error. The SDK's `APIError.error` is the body's `error` field, so
//! the openai-completions cases answer with `{ "error": parsedBody }`.
//!
//! The Bedrock cases mock `BedrockRuntimeClient.send()` to reject with a hand-built error. The port answers at the
//! HTTP level and lets `api::bedrock_client` build the error, so the assertions follow what the real SDK
//! (`@aws-sdk/client-bedrock-runtime` 3.1126.0) produces for the same response, recorded with pi's TS stream against
//! a local server:
//! - "bedrock (body-blind) surfaces the gateway body instead of Unknown: UnknownError": a 403 with
//!   `{"message":"blocked by gateway WAF"}` and no `x-amzn-errortype` gives `Unknown: blocked by gateway WAF`. The
//!   real SDK folds the body's `message` into the error and leaves `$response.body` as a consumed stream, so the
//!   status passthrough in `formatBedrockError` never fires and the message carries no `403`. The port drops the
//!   `toContain("403")` assertion and keeps the other two.
//! - "bedrock preserves the SDK validation message when the response body is a stream": a 400 with
//!   `x-amzn-errortype: ValidationException` and the message as its JSON body.

mod bedrock_support;
mod openai_responses_support;

use openai_responses_support::{context, json_response, model, responses_options};
use pi_ai::api::bedrock_converse_stream::stream_simple as stream_simple_bedrock;
use pi_ai::api::openai_completions::{OpenAICompletionsOptions, stream as stream_open_ai_completions};
use pi_ai::api::openai_responses::stream as stream_open_ai_responses;
use pi_ai::types::StopReason;
use pi_host::testing::TestHost;
use pi_host::testing::run;
use serde_json::json;

mod provider_error_body_passthrough_per_tier_regression {
	use super::*;

	/// `completionsModel`.
	fn completions_model() -> pi_ai::types::Model {
		model(json!({
			"id": "test-model",
			"name": "Test Model",
			"api": "openai-completions",
			"provider": "openrouter",
			"baseUrl": "https://openrouter.ai/api/v1",
			"reasoning": false,
			"input": ["text"],
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
			"contextWindow": 1000,
			"maxTokens": 100,
		}))
	}

	/// `drainResult(streamOpenAICompletions(completionsModel, context, { apiKey: "test" }))` against a 403 whose
	/// `error` field is `parsed_body`.
	async fn drain_completions(host: &TestHost, parsed_body: serde_json::Value) -> pi_ai::types::AssistantMessage {
		let body = json!({ "error": parsed_body });
		host.on_fetch(move |_| {
			let response = json_response(403, &body);
			Box::pin(async move { Ok(response) })
		});
		let mut options = OpenAICompletionsOptions::default();
		options.api_key = Some("test".into());
		let stream = stream_open_ai_completions(&completions_model(), &regression_context(), Some(options));
		while stream.next().await.is_some() {}
		stream.result().await.expect("stream has a result")
	}

	fn regression_context() -> pi_ai::types::TranscriptContext {
		context(json!({
			"systemPrompt": "",
			"messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }], "timestamp": 0 }],
			"tools": [],
		}))
	}

	#[test]
	fn openai_completions_body_blind_text_surfaces_status_body() {
		run(|host| async move {
			let output = drain_completions(&host, json!({ "error": "blocked by gateway WAF" })).await;

			assert_eq!(output.stop_reason, StopReason::Error);
			let error_message = output.error_message.unwrap_or_default();
			assert!(error_message.contains("403"), "{error_message}");
			assert!(error_message.contains("blocked by gateway WAF"), "{error_message}");
			assert_ne!(error_message, "403 status code (no body)");
		});
	}

	#[test]
	fn openai_completions_does_not_double_print_the_open_router_metadata_raw_extra() {
		run(|host| async move {
			// OpenRouter returns the extra reason under error.error.metadata.raw, which
			// is part of the parsed body normalizeProviderError already surfaces. The
			// manual append must not duplicate it.
			let output = drain_completions(
				&host,
				json!({
					"message": "Provider returned error",
					"code": 403,
					"metadata": { "raw": "upstream WAF blocked policy XYZ" },
				}),
			)
			.await;

			let error_message = output.error_message.unwrap_or_default();
			assert!(
				error_message.contains("upstream WAF blocked policy XYZ"),
				"{error_message}"
			);
			assert_eq!(
				error_message.matches("upstream WAF blocked policy XYZ").count(),
				1,
				"{error_message}"
			);
		});
	}

	#[test]
	fn openai_responses_status_only_keeps_the_prefix_and_surfaces_the_body() {
		run(|host| async move {
			host.on_fetch(|_| {
				Box::pin(async { Ok(json_response(403, &json!({ "error": "blocked by gateway WAF" }))) })
			});
			let responses_model = model(json!({
				"id": "gpt-test",
				"name": "GPT Test",
				"api": "openai-responses",
				"provider": "openai",
				"baseUrl": "https://api.openai.com/v1",
				"reasoning": false,
				"input": ["text"],
				"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
				"contextWindow": 1000,
				"maxTokens": 100,
			}));
			let context = context(json!({
				"systemPrompt": "",
				"messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }], "timestamp": 0 }],
				"tools": [],
			}));
			let stream = stream_open_ai_responses(
				&responses_model,
				&context,
				Some(responses_options(|options| options.api_key = Some("test".into()))),
			);
			while stream.next().await.is_some() {}
			let output = stream.result().await.expect("stream has a result");

			assert_eq!(output.stop_reason, StopReason::Error);
			let error_message = output.error_message.unwrap_or_default();
			assert!(error_message.contains("OpenAI API error (403)"), "{error_message}");
			assert!(error_message.contains("blocked by gateway WAF"), "{error_message}");
		});
	}

	/// Static AWS credentials in the process env, which the mocked Bedrock client did not need.
	fn set_bedrock_credentials(host: &TestHost) {
		host.set_env("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE");
		host.set_env("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
		host.set_env("AWS_REGION", "us-west-2");
	}

	fn bedrock_context() -> pi_ai::types::TranscriptContext {
		bedrock_support::context(json!({
			"messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }], "timestamp": 0 }],
		}))
	}

	#[test]
	fn bedrock_body_blind_surfaces_the_gateway_body_instead_of_unknown_unknown_error() {
		run(|host| async move {
			set_bedrock_credentials(&host);
			bedrock_support::serve(
				&host,
				vec![bedrock_support::json_response(
					403,
					&[("content-type", "application/json")],
					r#"{"message":"blocked by gateway WAF"}"#,
				)],
			);
			let model = bedrock_support::builtin_model("us.anthropic.claude-opus-4-8");
			let (_, output) = bedrock_support::collect(stream_simple_bedrock(&model, &bedrock_context(), None)).await;

			assert_eq!(output.stop_reason, StopReason::Error);
			let error_message = output.error_message.unwrap_or_default();
			assert!(error_message.contains("blocked by gateway WAF"), "{error_message}");
			assert!(!error_message.contains("Unknown: UnknownError"), "{error_message}");
			assert_eq!(error_message, "Unknown: blocked by gateway WAF");
		});
	}

	#[test]
	fn bedrock_preserves_the_sdk_validation_message_when_the_response_body_is_a_stream() {
		run(|host| async move {
			set_bedrock_credentials(&host);
			bedrock_support::serve(
				&host,
				vec![bedrock_support::json_response(
					400,
					&[
						("content-type", "application/json"),
						("x-amzn-errortype", "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/"),
						("x-amzn-requestid", "rid"),
					],
					&json!({
						"message": "Invocation of model ID anthropic.claude-opus-5 with on-demand throughput isn't supported. Retry with an inference profile.",
					})
					.to_string(),
				)],
			);
			let model = bedrock_support::builtin_model("global.anthropic.claude-opus-5");
			let (_, output) = bedrock_support::collect(stream_simple_bedrock(&model, &bedrock_context(), None)).await;

			assert_eq!(output.stop_reason, StopReason::Error);
			let error_message = output.error_message.unwrap_or_default();
			assert_eq!(
				error_message,
				"Validation error: Invocation of model ID anthropic.claude-opus-5 with on-demand throughput isn't supported. Retry with an inference profile."
			);
			assert!(!error_message.contains("_readableState"), "{error_message}");
		});
	}
}
