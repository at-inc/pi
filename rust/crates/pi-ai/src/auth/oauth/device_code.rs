//! Port of `packages/ai/src/auth/oauth/device-code.ts`.
//! PORT-STATUS: ported
//!
//! `OAuthDeviceCodePollResult<T>` is one enum: the TS union of the incomplete results and `complete`.

use std::rc::Rc;

use futures::future::{Either, select};
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

pub async fn poll_oauth_device_code_flow<T>(options: OAuthDeviceCodePollOptions<T>) -> PiResult<T> {
	let deadline = match options.expires_in_seconds {
		Some(expires_in_seconds) => pi_host::now_ms() + expires_in_seconds * 1000.0,
		None => f64::INFINITY,
	};
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

	while pi_host::now_ms() < deadline {
		if options.signal.aborted() {
			return Err(PiError::new(CANCEL_MESSAGE));
		}

		match (options.poll)().await? {
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

/// JS `Math.max(a, b)`: NaN when either is NaN.
fn math_max(a: f64, b: f64) -> f64 {
	if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// JS `Math.min(a, b)`: NaN when either is NaN.
fn math_min(a: f64, b: f64) -> f64 {
	if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}
