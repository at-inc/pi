//! Port of `packages/ai/test/openai-completions-retry.test.ts`.
//!
//! The TS mocks the SDK and checks that every `create` call gets `maxRetries: 0`. The port mocks HTTP: with the SDK
//! retries disabled, a retryable failure sends exactly one request unless provider retries are on, and the waits
//! between requests are the provider retry delays alone. Fake timers are the test host's virtual clock.

mod openai_completions_support;

use std::cell::RefCell;
use std::rc::Rc;

use openai_completions_support::{context, error_response, model, options, sse_response, user_message};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::{AssistantMessage, Model, StopReason};
use pi_host::HttpResponse;
use pi_host::testing::{TestHost, run};
use serde_json::json;

fn test_model() -> Model {
	let mut model = model("test-model", "opencode-go", "https://opencode.ai/zen/go/v1");
	model.context_window = 1000.0;
	model.max_tokens = 100.0;
	model
}

/// Serves `errors` in order, then the successful stream, recording the virtual time of each request.
fn serve(host: &TestHost, errors: Vec<HttpResponse>) -> Rc<RefCell<Vec<f64>>> {
	let times: Rc<RefCell<Vec<f64>>> = Rc::default();
	let recorded = times.clone();
	let errors = Rc::new(RefCell::new(errors));
	host.on_fetch(move |_| {
		recorded.borrow_mut().push(pi_host::now_ms());
		let response = if errors.borrow().is_empty() {
			sse_response(&[
				json!({ "id": "chatcmpl-test", "choices": [{ "index": 0, "delta": { "content": "ok" } }] }),
				json!({ "id": "chatcmpl-test", "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] }),
			])
		} else {
			errors.borrow_mut().remove(0)
		};
		Box::pin(async move { Ok(response) })
	});
	times
}

async fn consume(max_retries: Option<f64>, max_retry_delay_ms: Option<f64>) -> AssistantMessage {
	let mut request = options("test");
	request.max_retries = max_retries;
	request.max_retry_delay_ms = max_retry_delay_ms;
	stream(&test_model(), &context(vec![user_message("hi")]), Some(request))
		.result()
		.await
		.unwrap()
}

mod openai_completions_provider_retries {
	use super::*;

	#[test]
	fn disables_sdk_retries_by_default() {
		run(|host| async move {
			let times = serve(
				&host,
				vec![error_response(500, &[], r#"{"error":{"message":"server error"}}"#)],
			);

			let result = consume(None, None).await;

			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(times.borrow().len(), 1);
		});
	}

	#[test]
	fn honors_provider_retries_while_keeping_sdk_retries_disabled() {
		run(|host| async move {
			let times = serve(
				&host,
				vec![
					error_response(
						429,
						&[("retry-after-ms", "100")],
						r#"{"error":{"message":"rate limited"}}"#,
					),
					error_response(
						500,
						&[("retry-after-ms", "100")],
						r#"{"error":{"message":"server error"}}"#,
					),
				],
			);

			let result = consume(Some(2.0), Some(100.0)).await;

			assert_eq!(result.stop_reason, StopReason::Stop);
			let times = times.borrow();
			assert_eq!(times.len(), 3);
			assert_eq!(times[1] - times[0], 100.0);
			assert_eq!(times[2] - times[1], 100.0);
		});
	}

	#[test]
	fn fails_immediately_when_a_provider_requested_retry_delay_exceeds_the_limit() {
		run(|host| async move {
			let times = serve(
				&host,
				vec![error_response(
					429,
					&[("retry-after", "277403")],
					r#"{"error":{"message":"rate limited"}}"#,
				)],
			);

			let result = consume(Some(2.0), Some(1000.0)).await;

			assert_eq!(result.stop_reason, StopReason::Error);
			let error_message = result.error_message.unwrap();
			assert!(
				error_message.contains("Server requested 277403s retry delay (max: 1s)"),
				"{error_message}"
			);
			assert!(error_message.contains("rate limited"), "{error_message}");
			assert_eq!(times.borrow().len(), 1);
		});
	}
}
