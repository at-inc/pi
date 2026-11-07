//! Port of `packages/ai/src/utils/retry.ts`.
//! PORT-STATUS: ported
//!
//! The patterns are compiled by pi-schema's [`JsRegex`] from the TypeScript sources, so the lookarounds and `\s`,
//! `\S` keep their JavaScript meaning (case-insensitive matching uses Unicode simple case folding, see `JsRegex`).
//! `retryDelayMs` takes the whole `RetryPolicy` where TS takes `Pick<RetryPolicy, "baseDelayMs" | "maxAgentDelayMs">`.
//! The `RetryCallbacks` callbacks are async (`void | Promise<void>` is always awaited in TS) and may fail, like a
//! callback that throws; `produce` may fail like a rejected promise. The backoff sleep is `pi_host::sleep`, which
//! clamps like `setTimeout`; its abort is the TS `RetrySleepAbortError`. `RetryPolicy` deserializes settings JSON the
//! way TS reads it: `enabled` by truthiness and a missing number as NaN (TS `undefined`); a number field holding
//! another JSON type still fails to deserialize.

use std::future::Future;
use std::rc::Rc;
use std::sync::LazyLock;

use pi_host::{AbortSignal, LocalBoxFuture, PiResult};
use pi_schema::shared::JsRegex;
use serde::{Deserialize, Serialize};

use crate::types::{AssistantMessage, StopReason};

fn build_provider_error_pattern(patterns: &[&str]) -> JsRegex {
	JsRegex::new(&patterns.join("|"), "i").expect("retry patterns are valid regular expressions")
}

static NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN: LazyLock<JsRegex> = LazyLock::new(|| {
	build_provider_error_pattern(&[
		// OpenCode Go/free-tier limits returned as 429 JSON error types by OpenCode's
		// Zen API. These are subscription/account limits, not transient throttles.
		"GoUsageLimitError",
		"FreeUsageLimitError",
		// OpenCode Go subscription-limit text asks users to enable available-balance
		// usage after rolling/weekly/monthly limits are reached.
		"Monthly usage limit reached",
		"available balance",
		// Generic quota/budget/billing exhaustion. `insufficient_quota` is OpenAI's
		// quota/billing error code; the other strings cover common gateway wording.
		"insufficient_quota",
		"out of budget",
		"quota exceeded",
		"billing",
		// Sign in with ChatGPT: the subscription's shared usage limit, which resets
		// after hours rather than seconds.
		"subscription_sharing_usage_limit_exceeded",
		// Sign in with ChatGPT on a ChatGPT Business plan: a usage limit set by the
		// workspace admin. Only the admin can raise it.
		"subscription_sharing_workplace_individual_usage_limit_exceeded",
		// Zhipu/Z.ai (open.bigmodel.cn) error code 1308: the plan's usage limit is
		// reached, sent as HTTP 429. It resets after hours, not seconds (#11537).
		r#""code"\s*:\s*"?1308(?![0-9])"#,
	])
});

static RETRYABLE_PROVIDER_ERROR_PATTERN: LazyLock<JsRegex> = LazyLock::new(|| {
	build_provider_error_pattern(&[
		// Generic provider load, HTTP status, and server-side transient failures.
		"overloaded",
		"server_busy",
		"servers are currently busy",
		"currently experiencing high demand",
		"model is at capacity",
		"rate.?limit",
		"too many requests",
		// Status codes must not be part of a longer word or number, e.g. a request id or byte count (#11057).
		"(?<![a-z0-9])(?:429|500|502|503|504|520|524)(?![a-z0-9])",
		// Also matches "The service is temporarily unavailable" (#10965).
		"service.?(?:is.?)?(?:temporarily.?)?unavailable",
		"server.?error",
		"internal.?error",
		// Wrapper/provider text for transient upstream failures, including OpenRouter
		// "Provider returned error" responses (#2264) and gateway wording such as
		// "The upstream provider returned an error" (#10842).
		"provider.?returned.?(?:an.?)?error",
		"exceeded request buffer limit while retrying upstream",
		// Gateway failure relaying the upstream response stream, sent as an OpenAI
		// Responses `response.failed` event without an error code (#10920).
		"unable to forward upstream",
		// Other gateway upstream failures relayed as `response.failed` error codes (#10920).
		"upstream.?request.?failed",
		"upstream_stream_read_error",
		"stream was interrupted",
		"http/?2 stream failed",
		// Network, proxy, and fetch transport failures. This includes OpenAI Codex
		// raw-fetch failures such as "upstream connect", "connection refused", and
		// "reset before headers" (#733), plus OpenRouter connection drops (#3317).
		"network.?error",
		"connection.?error",
		"connection.?refused",
		"connection.?lost",
		"other side closed",
		"fetch failed",
		// node-fetch system errors, e.g. the Google ADC OAuth token refresh used by
		// google-vertex. The reason is often empty (#10996).
		r"request to \S+ failed, reason:",
		"getaddrinfo",
		"ENOTFOUND",
		"EAI_AGAIN",
		"upstream.?connect",
		"reset before headers",
		"socket hang up",
		"socket connection was closed",
		"timed? out",
		"timeout",
		"terminated",
		// WebSocket transports can report close/error text instead of HTTP/fetch text.
		"websocket.?closed",
		"websocket.?error",
		// Premature stream endings from SDKs and transports. Anthropic can throw
		// "stream ended without ..." and "Anthropic stream ended before message_stop"
		// (#4433); Bedrock/Smithy can throw an HTTP/2 no-response error (#3594).
		"ended without",
		"stream ended before message_stop",
		"stream ended before a terminal response event",
		"http2 request did not get a response",
		// Codex CLI mid-stream disconnect, relayed by proxies that wrap the Codex CLI,
		// e.g. "stream disconnected before completion: stream closed before response.completed" (#11057).
		"stream disconnected before completion",
		// Node ERR_HTTP2_STREAM_CANCEL: the HTTP/2 session died before the request was
		// sent, e.g. after the Bedrock SDK's 5-minute session timeout (#10379).
		"pending stream has been canceled",
		// Provider-requested retry delay cap failures should flow through the outer
		// retry policy so callers can surface/abort the backoff (#1123).
		"retry delay",
		// Explicit retry guidance emitted mid-stream by OpenAI Responses and Bedrock
		// stream exceptions (#6019).
		"you can retry your request",
		"try your request again",
		"please retry your request",
		// gRPC based providers (e.g. NVIDIA NIM)
		"ResourceExhausted",
		// Sign in with ChatGPT: usage or user data temporarily unavailable. Usage
		// failures can arrive mid-stream without an HTTP 503 in the message.
		"subscription_sharing_usage_unavailable",
		"subscription_sharing_user_unavailable",
	])
});

/// Retry policy: bounded attempts with exponential backoff (`baseDelayMs * 2^(attempt-1)`).
/// `maxAgentDelayMs` caps each computed delay and defaults to 60 seconds.
/// Matches `settings.retry` (`enabled`, `maxRetries`, `baseDelayMs`, `maxAgentDelayMs`) in coding-agent; kept
/// here so the classifier and the policy-driven retry loop live together and stay reusable
/// by the SDK and other callers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
	#[serde(default, deserialize_with = "js_truthy")]
	pub enabled: bool,
	/// Max retry attempts (0 = no retries). The initial call never counts as a retry.
	#[serde(default = "undefined_number")]
	pub max_retries: f64,
	/// Base delay in ms. Per-attempt delay is `baseDelayMs * 2^(attempt-1)` before jitter.
	#[serde(default = "undefined_number")]
	pub base_delay_ms: f64,
	/// Optional cap for agent-level retry delays in ms. Defaults to 60 seconds.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_agent_delay_ms: Option<f64>,
}

/// `policy?.enabled` is read for truthiness, so settings may hold any JSON value there.
fn js_truthy<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
	Ok(match serde_json::Value::deserialize(deserializer)? {
		serde_json::Value::Null | serde_json::Value::Bool(false) => false,
		serde_json::Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
		serde_json::Value::String(text) => !text.is_empty(),
		_ => true,
	})
}

/// A missing number reads as NaN, which behaves like TS `undefined` in the comparisons and arithmetic below: no
/// attempt reaches `attempt >= undefined`, and `undefined * 2 ** n` is NaN.
fn undefined_number() -> f64 {
	f64::NAN
}

pub const DEFAULT_MAX_AGENT_RETRY_DELAY_MS: f64 = 60_000.0;

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub fn retry_delay_ms(policy: &RetryPolicy, attempt: f64) -> f64 {
	let exponent = if attempt.is_nan() {
		f64::NAN
	} else {
		(attempt - 1.0).max(0.0)
	};
	let delay = policy.base_delay_ms * 2f64.powf(exponent);
	let safe_delay = if delay.fract() == 0.0 && delay.abs() <= MAX_SAFE_INTEGER {
		delay
	} else {
		MAX_SAFE_INTEGER
	};
	let cap = policy.max_agent_delay_ms.unwrap_or(DEFAULT_MAX_AGENT_RETRY_DELAY_MS);
	// `Math.min`: NaN wins, where `f64::min` would drop it.
	if cap.is_nan() { cap } else { safe_delay.min(cap) }
}

/// `onRetryScheduled(attempt, maxAttempts, delayMs, errorMessage)`.
pub type OnRetryScheduled = Rc<dyn Fn(f64, f64, f64, String) -> LocalBoxFuture<'static, PiResult<()>>>;
/// `onRetryAttemptStart()`.
pub type OnRetryAttemptStart = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<()>>>;
/// `onRetryFinished(success, attempt, finalError?)`.
pub type OnRetryFinished = Rc<dyn Fn(bool, f64, Option<String>) -> LocalBoxFuture<'static, PiResult<()>>>;

/// Optional callbacks emitted by [`retry_assistant_call`] around each retry.
#[derive(Clone, Default)]
pub struct RetryCallbacks {
	/// Emitted before the backoff sleep of each retry attempt (1-indexed).
	pub on_retry_scheduled: Option<OnRetryScheduled>,
	/// Emitted after the backoff sleep, immediately before the retried call starts.
	pub on_retry_attempt_start: Option<OnRetryAttemptStart>,
	/// Emitted once when the loop ends: success if a later call completed normally.
	pub on_retry_finished: Option<OnRetryFinished>,
}

async fn retry_finished(
	callbacks: Option<&RetryCallbacks>,
	success: bool,
	attempt: f64,
	final_error: Option<String>,
) -> PiResult<()> {
	match callbacks.and_then(|callbacks| callbacks.on_retry_finished.as_ref()) {
		Some(on_retry_finished) => on_retry_finished(success, attempt, final_error).await,
		None => Ok(()),
	}
}

/// Run a single assistant-producing call with bounded retry on transient errors.
///
/// Behavior:
/// - A successful response is returned immediately. Aborts are terminal and never
///   retried, but reported as unsuccessful if they happen after a retry was scheduled.
///   Aborts during the backoff sleep are normalized to an aborted `AssistantMessage`
///   too, so callers do not need to care when cancellation happened.
/// - A non-retryable error (per [`is_retryable_assistant_error`], including quota/
///   billing exhaustion) is returned immediately so deterministic errors fail fast.
/// - Otherwise retries up to `maxRetries` times with exponential backoff, emitting
///   `onRetryScheduled` before each sleep, `onRetryAttemptStart` after each sleep before
///   the retried call starts, and `onRetryFinished` once at the end (whether the loop
///   ends in success, exhausted retries, or an aborted backoff).
///
/// When `policy` is undefined or disabled, the first response is returned unchanged
/// (equivalent to calling `produce()` directly).
pub async fn retry_assistant_call<F, Fut>(
	mut produce: F,
	policy: Option<&RetryPolicy>,
	signal: Option<&AbortSignal>,
	callbacks: Option<&RetryCallbacks>,
) -> PiResult<AssistantMessage>
where
	F: FnMut() -> Fut,
	Fut: Future<Output = PiResult<AssistantMessage>>,
{
	let max_attempts = match policy {
		Some(policy) if policy.enabled => policy.max_retries,
		_ => 0.0,
	};

	let mut attempt = 0.0;
	let mut last_retry: Option<f64> = None;
	loop {
		let response = produce().await?;

		// Abort: terminal but not successful. Never retry an aborted message.
		if response.stop_reason == StopReason::Aborted {
			if let Some(last_attempt) = last_retry {
				retry_finished(callbacks, false, last_attempt, None).await?;
			}
			return Ok(response);
		}

		// Success: non-error, non-abort responses return as-is.
		if response.stop_reason != StopReason::Error {
			if let Some(last_attempt) = last_retry {
				retry_finished(callbacks, true, last_attempt, None).await?;
			}
			return Ok(response);
		}

		// Non-retryable, or budget exhausted: return the final error message.
		if attempt >= max_attempts || !is_retryable_assistant_error(&response) {
			if let Some(last_attempt) = last_retry {
				retry_finished(callbacks, false, last_attempt, response.error_message.clone()).await?;
			}
			return Ok(response);
		}

		attempt += 1.0;
		let error_message = response
			.error_message
			.clone()
			.filter(|message| !message.is_empty())
			.unwrap_or_else(|| "Unknown error".to_string());
		last_retry = Some(attempt);
		let delay_ms = retry_delay_ms(
			policy.expect("a retry is scheduled only with an enabled policy"),
			attempt,
		);
		if let Some(on_retry_scheduled) = callbacks.and_then(|callbacks| callbacks.on_retry_scheduled.as_ref()) {
			on_retry_scheduled(attempt, max_attempts, delay_ms, error_message.clone()).await?;
		}

		// Normalize aborts during retry backoff to the same AssistantMessage shape as
		// provider stream aborts, so callers do not need to care when cancellation happened.
		if pi_host::sleep(delay_ms, signal).await.is_err() {
			retry_finished(callbacks, false, attempt, Some(error_message)).await?;
			return Ok(AssistantMessage {
				stop_reason: StopReason::Aborted,
				error_message: None,
				..response
			});
		}
		if let Some(on_retry_attempt_start) = callbacks.and_then(|callbacks| callbacks.on_retry_attempt_start.as_ref())
		{
			on_retry_attempt_start().await?;
		}
	}
}

/// Classifies whether a failed assistant message looks like a transient provider
/// or transport error, so callers can decide if the last assistant turn should be
/// restarted.
///
/// This does not implement retry policy. Callers should first handle context
/// overflow separately, then apply their own retry budget, backoff, and reporting
/// before restarting the assistant turn.
pub fn is_retryable_assistant_error(message: &AssistantMessage) -> bool {
	if message.stop_reason != StopReason::Error {
		return false;
	}
	let Some(error_message) = message.error_message.as_deref().filter(|text| !text.is_empty()) else {
		return false;
	};
	if NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN.test(error_message) {
		return false;
	}
	RETRYABLE_PROVIDER_ERROR_PATTERN.test(error_message)
}

#[cfg(test)]
mod policy_tests {
	use super::*;

	#[test]
	fn reads_a_settings_policy_like_the_ts_runtime() {
		let policy: RetryPolicy =
			serde_json::from_value(serde_json::json!({ "enabled": 1, "baseDelayMs": 1000 })).unwrap();
		assert!(policy.enabled);
		assert!(policy.max_retries.is_nan());
		let policy: RetryPolicy =
			serde_json::from_value(serde_json::json!({ "enabled": "", "maxRetries": 2, "baseDelayMs": 1 })).unwrap();
		assert!(!policy.enabled);
		assert_eq!(policy.max_retries, 2.0);
	}
}
