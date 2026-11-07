//! Port of `packages/ai/test/retry.test.ts`.
//!
//! Differences from the TS:
//! - An `it.each` table over error messages is one test fn that checks every row; a failing assertion names the row.
//! - `vi.fn` call records are `RefCell` lists the callbacks push to; the callbacks resolve at once, like the
//!   synchronous TS mocks.
//! - The classification messages carry `timestamp: 0` so those tests need no host for `Date.now()`; the timestamp does
//!   not affect classification.
//! - The backoff sleeps run on the test host's virtual clock, so the 10-second backoff in the abort test takes no real
//!   time. `vi.waitFor` is `wait_for`, which polls every 50 ms of host time for up to 1 s like vitest's defaults.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::channel::oneshot;
use pi_ai::providers::faux::{FauxAssistantMessageOptions, faux_assistant_message};
use pi_ai::types::{AssistantMessage, StopReason};
use pi_ai::utils::retry::{
	RetryCallbacks, RetryPolicy, is_retryable_assistant_error, retry_assistant_call, retry_delay_ms,
};
use pi_host::testing::run;
use pi_host::{AbortController, PiResult};
use serde_json::json;

const OPENAI_EXPLICIT_RETRY_MESSAGE: &str = "An error occurred while processing your request. You can retry your request, or contact us through our help center at help.openai.com if the error persists. Please include the request ID req_******** in your message.";
const BEDROCK_EXPLICIT_RETRY_MESSAGE: &str =
	r#"{"message":"The system encountered an unexpected error during processing. Try your request again."}"#;
const NVIDIA_NIM_RESOURCE_EXHAUSTED_MESSAGE: &str =
	"ResourceExhausted: Worker local total request limit reached (288/48)";
const BUN_FETCH_SOCKET_CLOSED_MESSAGE: &str = "The socket connection was closed unexpectedly. For more information, pass `verbose: true` in the second argument to fetch()";
const OPENAI_RESPONSES_EARLY_EOF_MESSAGE: &str = "OpenAI Responses stream ended before a terminal response event";
const WRAPPED_DNS_LOOKUP_ERROR: &str =
	"The pending stream has been canceled (caused by: getaddrinfo ENOTFOUND bedrock-runtime.us-east-1.amazonaws.com)";
const AZURE_PEAK_LOAD_ERROR: &str = "The system is currently experiencing high demand and cannot process your request. Your request exceeds the maximum usage size allowed during peak load. For improved capacity reliability, consider switching to Provisioned Throughput.";

/// `fauxAssistantMessage("", { stopReason: "error", errorMessage })`.
fn error_message(error_message: &str) -> AssistantMessage {
	faux_assistant_message(
		"",
		Some(FauxAssistantMessageOptions {
			stop_reason: Some(StopReason::Error),
			error_message: Some(error_message.into()),
			timestamp: Some(0.0),
			..FauxAssistantMessageOptions::default()
		}),
	)
}

/// `fauxAssistantMessage("", { stopReason: "aborted" })`.
fn aborted_message() -> AssistantMessage {
	faux_assistant_message(
		"",
		Some(FauxAssistantMessageOptions {
			stop_reason: Some(StopReason::Aborted),
			..FauxAssistantMessageOptions::default()
		}),
	)
}

fn assert_retryable(messages: &[&str], expected: bool) {
	for message in messages {
		assert_eq!(
			is_retryable_assistant_error(&error_message(message)),
			expected,
			"{message}"
		);
	}
}

mod provider_retry_classification {
	use super::*;

	#[test]
	fn matches_explicit_provider_retry_guidance() {
		assert!(is_retryable_assistant_error(&error_message(
			OPENAI_EXPLICIT_RETRY_MESSAGE
		)));
		assert!(is_retryable_assistant_error(&error_message(
			BEDROCK_EXPLICIT_RETRY_MESSAGE
		)));
		assert!(is_retryable_assistant_error(&error_message(
			NVIDIA_NIM_RESOURCE_EXHAUSTED_MESSAGE
		)));
	}

	#[test]
	fn matches_bun_fetch_socket_drop_wording() {
		assert!(is_retryable_assistant_error(&error_message(
			BUN_FETCH_SOCKET_CLOSED_MESSAGE
		)));
	}

	#[test]
	fn matches_upstream_request_buffer_exhaustion_wording() {
		assert!(is_retryable_assistant_error(&error_message(
			"Error: exceeded request buffer limit while retrying upstream"
		)));
	}

	#[test]
	fn matches_dns_transport_failure_wording() {
		assert_retryable(
			&[
				WRAPPED_DNS_LOOKUP_ERROR,
				"connect ENOTFOUND api.example.com",
				"EAI_AGAIN api.example.com",
				"getaddrinfo failed for api.example.com",
			],
			true,
		);
	}

	#[test]
	fn matches_http2_pending_stream_cancellation() {
		// Regression for #10379.
		assert_retryable(
			&[
				"The pending stream has been canceled",
				"The pending stream has been canceled (caused by: socket closed)",
			],
			true,
		);
	}

	#[test]
	fn matches_service_unavailable_wording() {
		assert_retryable(
			&[
				// Regression for #10965 (provider unknown, no status code in the message).
				"The service is temporarily unavailable. Please retry later.",
				// Bedrock ServiceUnavailableException as formatted by formatBedrockError.
				"Service unavailable: The service is temporarily unavailable.",
			],
			true,
		);
	}

	#[test]
	fn matches_node_fetch_request_failure_wording() {
		assert_retryable(
			&[
				// Regression for #10996: node-fetch errors from the google-vertex OAuth token refresh.
				"request to https://oauth2.googleapis.com/token failed, reason:",
				"request to https://oauth2.googleapis.com/token failed, reason: connect ECONNREFUSED 127.0.0.1:9",
			],
			true,
		);
	}

	#[test]
	fn matches_gateway_upstream_failure_wording() {
		assert_retryable(
			&[
				// Regression for #10920: gateway upstream failures sent as `response.failed` error codes.
				"unknown: Unable to forward upstream stream",
				"upstream_error: Upstream request failed",
				"upstream_stream_read_error: Upstream response stream was interrupted",
				"Upstream HTTP/2 stream failed",
			],
			true,
		);
	}

	#[test]
	fn matches_openai_responses_streams_that_end_before_terminal_events() {
		assert!(is_retryable_assistant_error(&error_message(
			OPENAI_RESPONSES_EARLY_EOF_MESSAGE
		)));
	}

	#[test]
	fn matches_azure_peak_load_capacity_errors() {
		// Regression for #9669.
		assert!(is_retryable_assistant_error(&error_message(AZURE_PEAK_LOAD_ERROR)));
	}

	#[test]
	fn keeps_provider_limit_errors_non_retryable() {
		assert!(!is_retryable_assistant_error(&error_message("429 quota exceeded")));
	}

	#[test]
	fn keeps_the_chatgpt_subscription_usage_limit_non_retryable() {
		let error = r#"OpenAI API error (429): {"code":"subscription_sharing_usage_limit_exceeded","message":"Usage limit reached."}"#;
		assert!(!is_retryable_assistant_error(&error_message(error)));
	}

	#[test]
	fn keeps_the_chatgpt_business_workspace_usage_limit_non_retryable() {
		let error = r#"OpenAI API error (429): {"code":"subscription_sharing_workplace_individual_usage_limit_exceeded","message":"Usage limit reached."}"#;
		assert!(!is_retryable_assistant_error(&error_message(error)));
	}

	#[test]
	fn retries_temporary_chatgpt_subscription_errors() {
		assert_retryable(
			&[
				"subscription_sharing_usage_unavailable: Usage cannot be checked.",
				"subscription_sharing_user_unavailable: User cannot be loaded.",
			],
			true,
		);
	}

	#[test]
	fn classifies_assistant_error_messages() {
		assert!(is_retryable_assistant_error(&error_message("overloaded_error")));
		// Regression for #9627.
		assert!(is_retryable_assistant_error(&error_message(
			"520 status code (no body)"
		)));
		assert!(is_retryable_assistant_error(&error_message(
			"524 status code (no body)"
		)));
		assert!(!is_retryable_assistant_error(&faux_assistant_message(
			"not an error",
			Some(FauxAssistantMessageOptions {
				timestamp: Some(0.0),
				..FauxAssistantMessageOptions::default()
			})
		)));
	}

	#[test]
	fn does_not_match_status_codes_inside_ids_or_longer_numbers() {
		assert_retryable(
			&[
				// Regression for #11057: status codes inside ids or other numbers are not status codes.
				r#"401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_011CU503xyz"}"#,
				"400 image exceeds 5 MB maximum: 5316784 bytes > 5242880 bytes",
			],
			false,
		);
	}
}

mod retry_delay_ms {
	use super::*;

	fn policy(base_delay_ms: f64, max_agent_delay_ms: Option<f64>) -> RetryPolicy {
		RetryPolicy {
			enabled: false,
			max_retries: 0.0,
			base_delay_ms,
			max_agent_delay_ms,
		}
	}

	#[test]
	fn caps_agent_retry_delay() {
		// Regression for #8826.
		assert_eq!(retry_delay_ms(&policy(2000.0, None), 6.0), 60000.0);
		assert_eq!(retry_delay_ms(&policy(2000.0, Some(5000.0)), 5.0), 5000.0);
		assert_eq!(retry_delay_ms(&policy(2000.0, Some(0.0)), 5.0), 0.0);
	}
}

mod retry_assistant_call {
	use super::*;

	const DISABLED: RetryPolicy = RetryPolicy {
		enabled: false,
		max_retries: 3.0,
		base_delay_ms: 0.0,
		max_agent_delay_ms: None,
	};
	const ENABLED: RetryPolicy = RetryPolicy {
		enabled: true,
		max_retries: 3.0,
		base_delay_ms: 0.0,
		max_agent_delay_ms: None,
	};

	/// `vi.fn(async () => respond(n))`, where `n` counts earlier calls.
	struct Produce {
		calls: Rc<Cell<usize>>,
		respond: Rc<dyn Fn(usize) -> AssistantMessage>,
	}

	impl Produce {
		fn new(respond: impl Fn(usize) -> AssistantMessage + 'static) -> Self {
			Self {
				calls: Rc::new(Cell::new(0)),
				respond: Rc::new(respond),
			}
		}

		fn call(&self) -> impl FnMut() -> futures::future::Ready<PiResult<AssistantMessage>> + 'static {
			let (calls, respond) = (self.calls.clone(), self.respond.clone());
			move || {
				let n = calls.get();
				calls.set(n + 1);
				futures::future::ready(Ok(respond(n)))
			}
		}
	}

	/// `onRetryScheduled(attempt, maxAttempts, delayMs, errorMessage)` arguments.
	type ScheduledCall = (f64, f64, f64, String);
	/// `onRetryFinished(success, attempt, finalError)` arguments.
	type FinishedCall = (bool, f64, Option<String>);

	/// Recorded `onRetryScheduled` and `onRetryFinished` calls, and the callbacks that record them.
	#[derive(Default)]
	struct Recorded {
		scheduled: Rc<RefCell<Vec<ScheduledCall>>>,
		attempt_starts: Rc<Cell<usize>>,
		finished: Rc<RefCell<Vec<FinishedCall>>>,
	}

	impl Recorded {
		fn callbacks(&self) -> RetryCallbacks {
			let (scheduled, attempt_starts, finished) = (
				self.scheduled.clone(),
				self.attempt_starts.clone(),
				self.finished.clone(),
			);
			RetryCallbacks {
				on_retry_scheduled: Some(Rc::new(move |attempt, max_attempts, delay_ms, error_message| {
					scheduled
						.borrow_mut()
						.push((attempt, max_attempts, delay_ms, error_message));
					Box::pin(async { Ok(()) })
				})),
				on_retry_attempt_start: Some(Rc::new(move || {
					attempt_starts.set(attempt_starts.get() + 1);
					Box::pin(async { Ok(()) })
				})),
				on_retry_finished: Some(Rc::new(move |success, attempt, final_error| {
					finished.borrow_mut().push((success, attempt, final_error));
					Box::pin(async { Ok(()) })
				})),
			}
		}
	}

	fn terminated() -> AssistantMessage {
		error_message("terminated")
	}

	#[test]
	fn returns_a_successful_response_immediately_without_retrying() {
		run(|_| async {
			let produce = Produce::new(|_| faux_assistant_message("ok", None));
			let res = retry_assistant_call(produce.call(), Some(&ENABLED), None, None)
				.await
				.unwrap();
			assert_eq!(
				serde_json::to_value(&res.content).unwrap(),
				json!([{ "type": "text", "text": "ok" }])
			);
			assert_eq!(produce.calls.get(), 1);
		});
	}

	#[test]
	fn does_not_retry_an_aborted_message() {
		run(|_| async {
			let produce = Produce::new(|_| aborted_message());
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_scheduled: recorded.callbacks().on_retry_scheduled,
				..RetryCallbacks::default()
			};
			let res = retry_assistant_call(produce.call(), Some(&ENABLED), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(res.stop_reason, StopReason::Aborted);
			assert_eq!(produce.calls.get(), 1);
			assert!(recorded.scheduled.borrow().is_empty());
		});
	}

	#[test]
	fn does_not_retry_a_non_retryable_error_quota_billing() {
		run(|_| async {
			let produce = Produce::new(|_| error_message("insufficient_quota"));
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_attempt_start: None,
				..recorded.callbacks()
			};
			let res = retry_assistant_call(produce.call(), Some(&ENABLED), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(res.stop_reason, StopReason::Error);
			assert_eq!(produce.calls.get(), 1);
			assert!(recorded.scheduled.borrow().is_empty());
			assert!(recorded.finished.borrow().is_empty());
		});
	}

	#[test]
	fn retries_a_transient_error_up_to_max_retries_then_returns_the_final_error() {
		run(|_| async {
			let produce = Produce::new(|_| terminated());
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_attempt_start: None,
				..recorded.callbacks()
			};
			let res = retry_assistant_call(produce.call(), Some(&ENABLED), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(res.stop_reason, StopReason::Error);
			assert_eq!(produce.calls.get(), 4); // 1 initial + 3 retries
			assert_eq!(recorded.scheduled.borrow().len(), 3);
			assert!(
				recorded
					.finished
					.borrow()
					.contains(&(false, 3.0, Some("terminated".into())))
			);
		});
	}

	#[test]
	fn reports_capped_retry_delays() {
		// Regression for #8826.
		run(|_| async {
			let policy = RetryPolicy {
				enabled: true,
				max_retries: 4.0,
				base_delay_ms: 10.0,
				max_agent_delay_ms: Some(15.0),
			};
			let produce = Produce::new(|n| {
				if n + 1 < 5 {
					terminated()
				} else {
					faux_assistant_message("recovered", None)
				}
			});
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_scheduled: recorded.callbacks().on_retry_scheduled,
				..RetryCallbacks::default()
			};
			retry_assistant_call(produce.call(), Some(&policy), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(
				recorded
					.scheduled
					.borrow()
					.iter()
					.map(|(_, _, delay_ms, _)| *delay_ms)
					.collect::<Vec<_>>(),
				vec![10.0, 15.0, 15.0, 15.0]
			);
		});
	}

	#[test]
	fn stops_retrying_once_a_call_succeeds() {
		run(|_| async {
			let produce = Produce::new(|n| {
				if n + 1 < 3 {
					terminated()
				} else {
					faux_assistant_message("recovered", None)
				}
			});
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_finished: recorded.callbacks().on_retry_finished,
				..RetryCallbacks::default()
			};
			let res = retry_assistant_call(produce.call(), Some(&ENABLED), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(
				serde_json::to_value(&res.content).unwrap(),
				json!([{ "type": "text", "text": "recovered" }])
			);
			assert_eq!(produce.calls.get(), 3);
			assert!(recorded.finished.borrow().contains(&(true, 2.0, None)));
		});
	}

	#[test]
	fn reports_an_aborted_retried_call_as_unsuccessful() {
		run(|_| async {
			let produce = Produce::new(|n| if n == 0 { terminated() } else { aborted_message() });
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_finished: recorded.callbacks().on_retry_finished,
				..RetryCallbacks::default()
			};
			let res = retry_assistant_call(produce.call(), Some(&ENABLED), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(res.stop_reason, StopReason::Aborted);
			assert_eq!(produce.calls.get(), 2);
			assert!(recorded.finished.borrow().contains(&(false, 1.0, None)));
		});
	}

	#[test]
	fn does_not_retry_when_policy_is_disabled() {
		run(|_| async {
			let produce = Produce::new(|_| terminated());
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_attempt_start: None,
				..recorded.callbacks()
			};
			let res = retry_assistant_call(produce.call(), Some(&DISABLED), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(res.stop_reason, StopReason::Error);
			assert_eq!(produce.calls.get(), 1);
			assert!(recorded.scheduled.borrow().is_empty());
			assert!(recorded.finished.borrow().is_empty());
		});
	}

	#[test]
	fn emits_on_retry_attempt_start_after_backoff_before_each_retried_call() {
		run(|_| async {
			let events = Rc::new(RefCell::new(Vec::<String>::new()));
			let produce = {
				let events = events.clone();
				Produce::new(move |n| {
					events.borrow_mut().push(format!("produce:{n}"));
					if n + 1 < 3 {
						terminated()
					} else {
						faux_assistant_message("recovered", None)
					}
				})
			};
			let (scheduled, attempt_starts) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
			let callbacks = {
				let (scheduled, attempt_starts) = (scheduled.clone(), attempt_starts.clone());
				let (scheduled_events, attempt_events) = (events.clone(), events.clone());
				RetryCallbacks {
					on_retry_scheduled: Some(Rc::new(move |attempt, _, _, _| {
						scheduled.set(scheduled.get() + 1);
						scheduled_events.borrow_mut().push(format!("retry:{attempt}"));
						Box::pin(async { Ok(()) })
					})),
					on_retry_attempt_start: Some(Rc::new(move || {
						attempt_starts.set(attempt_starts.get() + 1);
						attempt_events.borrow_mut().push("attempt-start".into());
						Box::pin(async { Ok(()) })
					})),
					on_retry_finished: None,
				}
			};
			let res = retry_assistant_call(produce.call(), Some(&ENABLED), None, Some(&callbacks))
				.await
				.unwrap();
			assert_eq!(
				serde_json::to_value(&res.content).unwrap(),
				json!([{ "type": "text", "text": "recovered" }])
			);
			assert_eq!(scheduled.get(), 2);
			assert_eq!(attempt_starts.get(), 2);
			assert_eq!(
				*events.borrow(),
				[
					"produce:0",
					"retry:1",
					"attempt-start",
					"produce:1",
					"retry:2",
					"attempt-start",
					"produce:2",
				]
			);
		});
	}

	#[test]
	fn aborts_backoff_sleep_via_signal_returns_an_aborted_message_and_emits_on_retry_finished_false() {
		run(|_| async {
			let controller = AbortController::new();
			let produce = Produce::new(|_| terminated());
			let policy = RetryPolicy {
				enabled: true,
				max_retries: 5.0,
				base_delay_ms: 10_000.0,
				max_agent_delay_ms: None,
			};
			let recorded = Recorded::default();
			let callbacks = RetryCallbacks {
				on_retry_finished: recorded.callbacks().on_retry_finished,
				..RetryCallbacks::default()
			};
			let (sender, receiver) = oneshot::channel();
			{
				let (call, signal) = (produce.call(), controller.signal());
				pi_host::spawn_local(async move {
					let res = retry_assistant_call(call, Some(&policy), Some(&signal), Some(&callbacks)).await;
					let _ = sender.send(res);
				});
			}
			// Let one error call resolve and the first backoff sleep start, then abort.
			wait_for(|| produce.calls.get() > 0).await;
			controller.abort();
			let res = receiver.await.unwrap().unwrap();
			assert_eq!(res.stop_reason, StopReason::Aborted);
			assert_eq!(res.error_message, None);
			assert_eq!(produce.calls.get(), 1);
			assert!(
				recorded
					.finished
					.borrow()
					.contains(&(false, 1.0, Some("terminated".into())))
			);
		});
	}

	/// `vi.waitFor(check)` with vitest's defaults: retry every 50 ms for up to 1 s.
	async fn wait_for(check: impl Fn() -> bool) {
		for _ in 0..20 {
			if check() {
				return;
			}
			pi_host::sleep(50.0, None).await.unwrap();
		}
		assert!(check(), "wait_for timed out");
	}
}
