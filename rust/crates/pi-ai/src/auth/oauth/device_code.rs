//! Port of `packages/ai/src/auth/oauth/device-code.ts`.
//! PORT-STATUS: reviewed
//!
//! `OAuthDeviceCodePollResult<T>` is one enum: the TS union of the incomplete results and `complete`.

use std::future::Future;
use std::rc::Rc;

use futures::channel::oneshot;
use futures::future::{Either, select};
use pi_host::js::{math_max, math_min};
use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};

const CANCEL_MESSAGE: &str = "Login cancelled";
const TIMEOUT_MESSAGE: &str = "Device flow timed out";
const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.";
const MINIMUM_INTERVAL_MS: f64 = 1000.0;
// RFC 8628 section 3.2: if the authorization server omits `interval`, the client must use 5 seconds.
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;
// RFC 8628 section 3.5: `slow_down` means the polling interval must increase by 5 seconds.
const SLOW_DOWN_INTERVAL_INCREMENT_MS: f64 = 5000.0;

#[derive(Clone, Debug, PartialEq)]
pub enum OAuthDeviceCodePollResult<T> {
	Pending,
	SlowDown { interval_seconds: Option<f64> },
	Failed { message: String },
	Complete { value: T },
}

/// `OAuthDeviceCodePollOptions.poll`.
pub type OAuthDeviceCodePollFunction<T> =
	Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<OAuthDeviceCodePollResult<T>>>>;

pub struct OAuthDeviceCodePollOptions<T> {
	pub interval_seconds: Option<f64>,
	pub expires_in_seconds: Option<f64>,
	pub wait_before_first_poll: Option<bool>,
	pub poll: OAuthDeviceCodePollFunction<T>,
	pub signal: AbortSignal,
}

pub async fn abortable_sleep(ms: f64, signal: &AbortSignal, cancel_message: &str) -> PiResult<()> {
	if signal.aborted() {
		return Err(PiError::new(cancel_message));
	}
	let aborted = signal.aborted_future();
	futures::pin_mut!(aborted);
	match select(pi_host::current().sleep(ms), aborted).await {
		Either::Left(_) => Ok(()),
		Either::Right(_) => Err(PiError::new(cancel_message)),
	}
}

/// Polls until `poll` completes or fails, the deadline passes, or `signal` aborts. Like the TS promise, the
/// deadline and the first `poll()` call happen when this is called, and the flow runs whether or not the returned
/// future is awaited.
pub fn poll_oauth_device_code_flow<T: 'static>(
	options: OAuthDeviceCodePollOptions<T>,
) -> impl Future<Output = PiResult<T>> + 'static + use<T> {
	let deadline = match options.expires_in_seconds {
		Some(expires_in_seconds) => pi_host::now_ms() + expires_in_seconds * 1000.0,
		None => f64::INFINITY,
	};
	let first_poll =
		(options.wait_before_first_poll != Some(true) && pi_host::now_ms() < deadline && !options.signal.aborted())
			.then(|| (options.poll)());
	let (sender, receiver) = oneshot::channel();
	pi_host::spawn_local(async move {
		let _ = sender.send(poll_until_done(options, deadline, first_poll).await);
	});
	async move {
		match receiver.await {
			Ok(result) => result,
			Err(_) => futures::future::pending().await,
		}
	}
}

async fn poll_until_done<T>(
	options: OAuthDeviceCodePollOptions<T>,
	deadline: f64,
	mut first_poll: Option<LocalBoxFuture<'static, PiResult<OAuthDeviceCodePollResult<T>>>>,
) -> PiResult<T> {
	let mut interval_ms = math_max(
		MINIMUM_INTERVAL_MS,
		(options.interval_seconds.unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS) * 1000.0).floor(),
	);

	let mut slow_down_responses = 0;
	if options.wait_before_first_poll == Some(true) {
		let remaining_ms = deadline - pi_host::now_ms();
		if remaining_ms > 0.0 {
			abortable_sleep(math_min(interval_ms, remaining_ms), &options.signal, CANCEL_MESSAGE).await?;
		}
	}

	loop {
		let poll = match first_poll.take() {
			Some(poll) => poll,
			None => {
				// `while (Date.now() < deadline)`: a NaN deadline ends the loop, which `now >= deadline` would not.
				if pi_host::now_ms().partial_cmp(&deadline) != Some(std::cmp::Ordering::Less) {
					break;
				}
				if options.signal.aborted() {
					return Err(PiError::new(CANCEL_MESSAGE));
				}
				(options.poll)()
			}
		};

		match poll.await? {
			OAuthDeviceCodePollResult::Complete { value } => return Ok(value),
			OAuthDeviceCodePollResult::Failed { message } => return Err(PiError::new(message)),
			OAuthDeviceCodePollResult::SlowDown { interval_seconds } => {
				slow_down_responses += 1;
				// Use the server-provided interval when given (GitHub reports the new required minimum
				// in `interval`); trusting only a client-tracked value risks polling early forever under
				// WSL/VM clock drift. Otherwise apply RFC 8628 section 3.5: increase by 5 seconds.
				interval_ms = match interval_seconds {
					Some(interval_seconds) if interval_seconds.is_finite() && interval_seconds > 0.0 => {
						math_max(MINIMUM_INTERVAL_MS, (interval_seconds * 1000.0).floor())
					}
					_ => math_max(MINIMUM_INTERVAL_MS, interval_ms + SLOW_DOWN_INTERVAL_INCREMENT_MS),
				};
			}
			OAuthDeviceCodePollResult::Pending => {}
		}

		let remaining_ms = deadline - pi_host::now_ms();
		if remaining_ms <= 0.0 {
			break;
		}

		abortable_sleep(math_min(interval_ms, remaining_ms), &options.signal, CANCEL_MESSAGE).await?;
	}

	Err(PiError::new(if slow_down_responses > 0 {
		SLOW_DOWN_TIMEOUT_MESSAGE
	} else {
		TIMEOUT_MESSAGE
	}))
}
