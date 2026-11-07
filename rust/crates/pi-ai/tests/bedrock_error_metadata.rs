//! Port of `packages/ai/test/bedrock-error-metadata.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@aws-sdk/client-bedrock-runtime` and makes `client.send()` reject or resolve with a hand-built
//!   stream. The port answers at the HTTP level and lets `api::bedrock_client` build the same errors: a mocked
//!   `BedrockRuntimeServiceException` with `$metadata` is a non-2xx response with `x-amzn-errortype`,
//!   `x-amzn-requestid` and a JSON `{ "message": ... }` body, and the `$metadata.requestId` of a resolved stream is
//!   its `x-amzn-requestid` header. Credentials come from `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` in the process
//!   env, which the mocked client did not need.
//! - "leaves errorMessage untouched so retry classification is unaffected": the TS attaches a `$response` with a
//!   pipeable body to check that `normalizeProviderError` does not fold it into the message. The Rust client does not
//!   keep `$response` (see the `bedrock_client` header), so the port sends the same 400 with its JSON body and checks
//!   the same message.
//! - "reports only the request id for a modeled mid-stream exception": the mock throws a bare object literal from the
//!   iterator. The real SDK never does: a modeled exception frame is thrown by the event stream unmarshaller as the
//!   named service exception (recorded against the real SDK in `bedrock_client.rs`, case `exception-frame`). The port
//!   sends a `throttlingException` frame after `messageStart`, so the details also carry
//!   `errorCode: "ThrottlingException"`; the request id still comes only from the response, since the exception has
//!   no `$metadata` of its own.
//! - "captures the error code for an unmodeled mid-stream error": the unmodeled `Error` named after `:error-code` is an
//!   `error` frame with `:error-code: ModelStreamErrorException`.
//! - "does not report a transport failure name as a provider error code": the `TimeoutError` is a response body that
//!   fails with a `TimeoutError` after the `messageStart` frame.
//! - "emits no diagnostic when the failure carries no provider metadata": the rejected `Error("socket hang up")` is a
//!   fetch that fails with that error.

mod bedrock_support;

use bedrock_support::{
	Reply, builtin_model, collect, error_message, event, exception, json_response, serve, stream_response,
};
use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{AssistantMessage, CacheRetention, Model, StopReason, StreamOptions, TranscriptContext};
use pi_host::testing::{TestHost, run};
use pi_host::{AbortController, AbortSignal, HttpResponse, PiError};
use serde_json::{Value, json};

const DIAGNOSTIC_TYPE: &str = "bedrock_response_failure";
const VALIDATION_MESSAGE: &str = "The provided model identifier is invalid.";
const REQUEST_ID: &str = "11111111-2222-3333-4444-555555555555";

fn context() -> TranscriptContext {
	bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }))
}

/// `getModelFixture()`.
fn get_model_fixture() -> Model {
	builtin_model("us.anthropic.claude-opus-4-8")
}

/// Static AWS credentials in the process env, which the default chain picks up.
fn set_credentials(host: &TestHost) {
	host.set_env("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE");
	host.set_env("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
}

/// `makeServiceException(name, { $metadata: { httpStatusCode: status, requestId } })` as the response the SDK's
/// `handleError` path builds it from.
fn service_exception_response(name: &str, status: u16, request_id: &str) -> Reply {
	json_response(
		status,
		&[
			("x-amzn-errortype", name),
			("x-amzn-requestid", request_id),
			("content-type", "application/json"),
		],
		&json!({ "message": VALIDATION_MESSAGE }).to_string(),
	)
}

/// `respondWithFailingStream(thrown)`: `messageStart`, then the frame that fails the stream.
fn respond_with_failing_stream(failure: Vec<u8>) -> Reply {
	stream_response(
		&[event("messageStart", json!({ "role": "assistant" })), failure],
		&[("x-amzn-requestid", REQUEST_ID)],
	)
}

/// `runBedrock(signal)`.
async fn run_bedrock(signal: Option<AbortSignal>) -> AssistantMessage {
	let mut options = BedrockOptions {
		base: StreamOptions {
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..BedrockOptions::default()
	};
	options.signal = signal;
	collect(stream(&get_model_fixture(), &context(), Some(options))).await.1
}

/// `findDiagnostic(message)` as JSON.
fn find_diagnostic(message: &AssistantMessage) -> Option<Value> {
	message
		.diagnostics
		.as_ref()?
		.iter()
		.find(|diagnostic| diagnostic.r#type == DIAGNOSTIC_TYPE)
		.map(|diagnostic| serde_json::to_value(diagnostic).unwrap())
}

mod bedrock_failure_diagnostics {
	use super::*;

	#[test]
	fn records_status_error_code_and_request_id_for_a_non_2xx_from_client_send() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![service_exception_response("ValidationException", 400, REQUEST_ID)],
			);

			let message = run_bedrock(None).await;
			let diagnostic = find_diagnostic(&message).expect("a diagnostic");

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(
				diagnostic["details"],
				json!({ "status": 400, "errorCode": "ValidationException", "requestId": REQUEST_ID })
			);
			assert_eq!(diagnostic.get("error"), None);
			let mut keys: Vec<&str> = diagnostic.as_object().unwrap().keys().map(String::as_str).collect();
			keys.sort();
			assert_eq!(keys, ["details", "timestamp", "type"]);
		});
	}

	#[test]
	fn leaves_error_message_untouched_so_retry_classification_is_unaffected() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![service_exception_response("ValidationException", 400, REQUEST_ID)],
			);

			assert_eq!(
				run_bedrock(None).await.error_message,
				Some(format!("Validation error: {VALIDATION_MESSAGE}"))
			);
		});
	}

	#[test]
	fn reports_only_the_request_id_for_a_modeled_mid_stream_exception() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![respond_with_failing_stream(exception(
					"throttlingException",
					json!({ "message": "Too many requests, please wait." }),
				))],
			);

			let message = run_bedrock(None).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(
				find_diagnostic(&message).expect("a diagnostic")["details"],
				json!({ "errorCode": "ThrottlingException", "requestId": REQUEST_ID })
			);
		});
	}

	#[test]
	fn captures_the_error_code_for_an_unmodeled_mid_stream_error() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![respond_with_failing_stream(error_message(
					"ModelStreamErrorException",
					"Model stream terminated unexpectedly.",
				))],
			);

			assert_eq!(
				find_diagnostic(&run_bedrock(None).await).expect("a diagnostic")["details"],
				json!({ "errorCode": "ModelStreamErrorException", "requestId": REQUEST_ID })
			);
		});
	}

	#[test]
	fn does_not_report_a_transport_failure_name_as_a_provider_error_code() {
		run(|host| async move {
			set_credentials(&host);
			host.on_fetch(|_| {
				let mut response = HttpResponse::from_bytes(
					200,
					IndexMap::from([
						(
							"content-type".to_string(),
							"application/vnd.amazon.eventstream".to_string(),
						),
						("x-amzn-requestid".to_string(), REQUEST_ID.to_string()),
					]),
					Vec::new(),
				);
				let message_start = event("messageStart", json!({ "role": "assistant" }));
				response.body = futures::stream::iter([
					Ok(message_start.into()),
					Err(PiError::named("TimeoutError", "Connection timed out after 1000 ms")),
				])
				.boxed_local();
				Box::pin(async move { Ok(response) })
			});

			assert_eq!(
				find_diagnostic(&run_bedrock(None).await).expect("a diagnostic")["details"],
				json!({ "requestId": REQUEST_ID })
			);
		});
	}

	#[test]
	fn emits_no_diagnostic_when_the_failure_carries_no_provider_metadata() {
		run(|host| async move {
			set_credentials(&host);
			host.on_fetch(|_| Box::pin(async { Err(PiError::new("socket hang up")) }));

			let message = run_bedrock(None).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.error_message.as_deref(), Some("socket hang up"));
			assert_eq!(find_diagnostic(&message), None);
		});
	}

	#[test]
	fn emits_no_diagnostic_for_an_aborted_turn() {
		run(|host| async move {
			set_credentials(&host);
			let controller = AbortController::new();
			controller.abort();
			serve(
				&host,
				vec![service_exception_response("ValidationException", 400, REQUEST_ID)],
			);

			let message = run_bedrock(Some(controller.signal())).await;

			assert_eq!(message.stop_reason, StopReason::Aborted);
			assert_eq!(find_diagnostic(&message), None);
		});
	}

	#[test]
	fn drops_header_derived_values_that_exceed_the_length_bound() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![service_exception_response(
					&format!("{}Exception", "E".repeat(5000)),
					400,
					&"R".repeat(5000),
				)],
			);

			assert_eq!(
				find_diagnostic(&run_bedrock(None).await).expect("a diagnostic")["details"],
				json!({ "status": 400 })
			);
		});
	}

	#[test]
	fn omits_the_sdks_unknown_placeholder_instead_of_reporting_it_as_a_code() {
		run(|host| async move {
			set_credentials(&host);
			serve(
				&host,
				vec![json_response(
					403,
					&[("x-amzn-requestid", REQUEST_ID), ("content-type", "application/json")],
					&json!({ "message": VALIDATION_MESSAGE }).to_string(),
				)],
			);

			assert_eq!(
				find_diagnostic(&run_bedrock(None).await).expect("a diagnostic")["details"],
				json!({ "status": 403, "requestId": REQUEST_ID })
			);
		});
	}
}
