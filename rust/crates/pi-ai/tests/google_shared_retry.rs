//! Port of `packages/ai/test/google-shared-retry.test.ts`.
//!
//! The SDK `ApiError` is a `PiError` with `status` and no `data`. Fake timers are the test host's virtual clock, which
//! fires the backoff timer as soon as the request waits on it, so `advanceTimersByTimeAsync(500)` is the check that
//! the virtual clock moved at most 500 ms.
//! `rejects.toBe(error)` compares the message and status of the error.

use std::cell::Cell;
use std::rc::Rc;

use pi_ai::api::google_shared::{GoogleRetryOptions, retry_google_request};
use pi_host::PiError;
use pi_host::testing::run;

/// Shaped like @google/genai's ApiError: has `status`, but no `headers`.
fn google_api_error(status: u16) -> PiError {
	PiError::new(format!("got status: {status}")).with_status(status)
}

mod google_request_retries {
	use super::*;

	#[test]
	fn retries_a_headers_less_sdk_error_with_a_retryable_status() {
		run(|_| async {
			let calls = Rc::new(Cell::new(0));
			let started_at = pi_host::now_ms();
			let result = retry_google_request(
				|| {
					let calls = calls.clone();
					async move {
						calls.set(calls.get() + 1);
						if calls.get() == 1 {
							Err(google_api_error(429))
						} else {
							Ok("ok")
						}
					}
				},
				Some(GoogleRetryOptions {
					max_retries: Some(1.0),
					..GoogleRetryOptions::default()
				}),
			)
			.await;

			assert!(pi_host::now_ms() - started_at <= 500.0);
			assert_eq!(result, Ok("ok"));
			assert_eq!(calls.get(), 2);
		});
	}

	#[test]
	fn does_not_retry_when_max_retries_is_unset() {
		run(|_| async {
			let calls = Rc::new(Cell::new(0));
			let result: Result<&str, PiError> = retry_google_request(
				|| {
					let calls = calls.clone();
					async move {
						calls.set(calls.get() + 1);
						Err(google_api_error(429))
					}
				},
				None,
			)
			.await;

			let error = result.unwrap_err();
			assert_eq!(error.message, "got status: 429");
			assert_eq!(error.status, Some(429));
			assert_eq!(calls.get(), 1);
		});
	}

	#[test]
	fn does_not_retry_a_non_retryable_status() {
		run(|_| async {
			let calls = Rc::new(Cell::new(0));
			let result: Result<&str, PiError> = retry_google_request(
				|| {
					let calls = calls.clone();
					async move {
						calls.set(calls.get() + 1);
						Err(google_api_error(400))
					}
				},
				Some(GoogleRetryOptions {
					max_retries: Some(2.0),
					..GoogleRetryOptions::default()
				}),
			)
			.await;

			let error = result.unwrap_err();
			assert_eq!(error.message, "got status: 400");
			assert_eq!(error.status, Some(400));
			assert_eq!(calls.get(), 1);
		});
	}
}
