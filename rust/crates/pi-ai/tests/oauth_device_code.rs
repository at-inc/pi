//! Port of `packages/ai/test/oauth-device-code.test.ts`.
//!
//! vitest advances fake timers step by step and checks `pollTimes` in between; `pi_host::testing` fires each timer
//! as soon as everything is stalled, so each test checks the recorded poll times, which pin the same schedule.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use pi_ai::auth::oauth::device_code::{
	OAuthDeviceCodePollOptions, OAuthDeviceCodePollResult, poll_oauth_device_code_flow,
};
use pi_host::{AbortController, PiError};

const START: f64 = 1_700_000_000_000.0;

fn scripted(
	results: Vec<OAuthDeviceCodePollResult<String>>,
	poll_times: Rc<RefCell<Vec<f64>>>,
) -> pi_ai::auth::oauth::device_code::OAuthDeviceCodePollFunction<String> {
	let results = Rc::new(RefCell::new(VecDeque::from(results)));
	Rc::new(move || {
		poll_times.borrow_mut().push(pi_host::now_ms());
		let result = results.borrow_mut().pop_front();
		Box::pin(async move { result.ok_or_else(|| PiError::new("Unexpected extra poll")) })
	})
}

mod oauth_device_code_polling {
	use super::*;

	#[test]
	fn polls_immediately_and_returns_the_completed_value() {
		pi_host::testing::run(|_| async {
			let poll_times = Rc::new(RefCell::new(Vec::new()));
			let result = poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
				interval_seconds: Some(2.0),
				expires_in_seconds: Some(30.0),
				wait_before_first_poll: None,
				poll: scripted(
					vec![
						OAuthDeviceCodePollResult::Pending,
						OAuthDeviceCodePollResult::Complete { value: "token".into() },
					],
					poll_times.clone(),
				),
				signal: AbortController::new().signal(),
			})
			.await;
			assert_eq!(result.unwrap(), "token");
			assert_eq!(*poll_times.borrow(), vec![START, START + 2000.0]);
		});
	}

	#[test]
	fn can_wait_before_the_first_poll() {
		pi_host::testing::run(|_| async {
			let poll_times = Rc::new(RefCell::new(Vec::new()));
			let result = poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
				interval_seconds: Some(2.0),
				expires_in_seconds: Some(30.0),
				wait_before_first_poll: Some(true),
				poll: scripted(
					vec![OAuthDeviceCodePollResult::Complete { value: "token".into() }],
					poll_times.clone(),
				),
				signal: AbortController::new().signal(),
			})
			.await;
			assert_eq!(result.unwrap(), "token");
			assert_eq!(*poll_times.borrow(), vec![START + 2000.0]);
		});
	}

	#[test]
	fn increases_the_interval_by_5_seconds_after_slow_down_without_a_server_interval() {
		pi_host::testing::run(|_| async {
			let poll_times = Rc::new(RefCell::new(Vec::new()));
			let result = poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
				interval_seconds: Some(2.0),
				expires_in_seconds: Some(900.0),
				wait_before_first_poll: None,
				poll: scripted(
					vec![
						OAuthDeviceCodePollResult::SlowDown { interval_seconds: None },
						OAuthDeviceCodePollResult::Complete { value: "token".into() },
					],
					poll_times.clone(),
				),
				signal: AbortController::new().signal(),
			})
			.await;
			assert_eq!(result.unwrap(), "token");
			assert_eq!(*poll_times.borrow(), vec![START, START + 7000.0]);
		});
	}

	#[test]
	fn honors_a_server_provided_slow_down_interval() {
		pi_host::testing::run(|_| async {
			let poll_times = Rc::new(RefCell::new(Vec::new()));
			let result = poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
				interval_seconds: Some(2.0),
				expires_in_seconds: Some(900.0),
				wait_before_first_poll: None,
				poll: scripted(
					vec![
						OAuthDeviceCodePollResult::SlowDown {
							interval_seconds: Some(30.0),
						},
						OAuthDeviceCodePollResult::Complete { value: "token".into() },
					],
					poll_times.clone(),
				),
				signal: AbortController::new().signal(),
			})
			.await;
			assert_eq!(result.unwrap(), "token");
			assert_eq!(*poll_times.borrow(), vec![START, START + 30000.0]);
		});
	}

	#[test]
	fn cancels_an_in_flight_wait() {
		pi_host::testing::run(|_| async {
			let controller = AbortController::new();
			let signal = controller.signal();
			let result = poll_oauth_device_code_flow::<String>(OAuthDeviceCodePollOptions {
				interval_seconds: Some(5.0),
				expires_in_seconds: Some(30.0),
				wait_before_first_poll: None,
				poll: Rc::new(move || {
					controller.abort();
					Box::pin(async { Ok(OAuthDeviceCodePollResult::Pending) })
				}),
				signal,
			})
			.await;
			assert_eq!(result.unwrap_err().message, "Login cancelled");
		});
	}
}
