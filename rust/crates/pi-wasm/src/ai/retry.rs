//! Exports of `utils/retry`. `retryAssistantCall` returns the message `produce` resolved with (the same object) and
//! rethrows what `produce` or a callback threw, as the TS does.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use pi_ai::utils::retry::{self, OnRetryAttemptStart, OnRetryFinished, OnRetryScheduled, RetryCallbacks, RetryPolicy};
use pi_ai::{AssistantMessage, StopReason};
use pi_host::{LocalBoxFuture, PiResult};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::overflow::assistant_facts;
use crate::options::arg;

/// `DEFAULT_MAX_AGENT_RETRY_DELAY_MS`.
#[wasm_bindgen(js_name = aiDefaultMaxAgentRetryDelayMs)]
pub fn default_max_agent_retry_delay_ms() -> f64 {
	retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS
}

/// `retryDelayMs(policy, attempt)`.
#[wasm_bindgen(js_name = aiRetryDelayMs)]
pub fn retry_delay_ms(policy: JsValue, attempt: f64) -> Result<f64, JsValue> {
	let policy: RetryPolicy = arg(policy)?;
	Ok(retry::retry_delay_ms(&policy, attempt))
}

/// `isRetryableAssistantError(message)`.
#[wasm_bindgen(js_name = aiIsRetryableAssistantError)]
pub fn is_retryable_assistant_error(message: JsValue) -> Result<bool, JsValue> {
	Ok(retry::is_retryable_assistant_error(&assistant_facts(message)?))
}

/// `retryAssistantCall(produce, policy, signal, callbacks)`.
#[wasm_bindgen(js_name = aiRetryAssistantCall)]
pub async fn retry_assistant_call(
	produce: Function,
	policy: JsValue,
	signal: JsValue,
	callbacks: JsValue,
) -> Result<JsValue, JsValue> {
	let policy: Option<RetryPolicy> = arg(policy)?;
	let signal = crate::abort::from_js(&signal);
	let calls = Calls::default();
	let callbacks = retry_callbacks(&callbacks, &calls);
	let last = Rc::new(RefCell::new(JsValue::UNDEFINED));
	let produce_calls = calls.clone();
	let produced = last.clone();
	let result = retry::retry_assistant_call(
		move || {
			let calls = produce_calls.clone();
			let produce = produce.clone();
			let produced = produced.clone();
			async move {
				let response = calls.call(&produce, &JsValue::UNDEFINED, &[]).await?;
				let message = assistant_facts(response.clone()).map_err(crate::error::from_js)?;
				*produced.borrow_mut() = response;
				Ok(message)
			}
		},
		policy.as_ref(),
		signal.as_ref(),
		Some(&callbacks),
	)
	.await;
	match result {
		Ok(message) => response_of(&message, &last.borrow()),
		Err(error) => Err(calls.thrown.take().unwrap_or_else(|| crate::error::to_js(&error))),
	}
}

/// The JS result for the message the loop returned: the last response itself, or for an abort during the backoff a copy
/// of it with `stopReason: "aborted"` and without `errorMessage`, as the TS spreads it.
fn response_of(message: &AssistantMessage, response: &JsValue) -> Result<JsValue, JsValue> {
	let response_stop = Reflect::get(response, &"stopReason".into())?.as_string();
	if message.stop_reason != StopReason::Aborted || response_stop.as_deref() == Some("aborted") {
		return Ok(response.clone());
	}
	let copy = Object::assign(&Object::new(), response.unchecked_ref());
	Reflect::delete_property(&copy, &"errorMessage".into())?;
	Reflect::set(&copy, &"stopReason".into(), &"aborted".into())?;
	Ok(copy.into())
}

/// JS calls of one retry loop; keeps the first value a call threw, so the loop rethrows that value itself.
#[derive(Clone, Default)]
struct Calls {
	thrown: Rc<RefCell<Option<JsValue>>>,
}

impl Calls {
	/// `await fn.call(this, ...args)`.
	async fn call(&self, function: &Function, this: &JsValue, args: &[JsValue]) -> PiResult<JsValue> {
		let args: js_sys::Array = args.iter().collect();
		let result = match function.apply(this, &args) {
			Ok(result) => JsFuture::from(Promise::resolve(&result)).await,
			Err(error) => Err(error),
		};
		result.map_err(|error| {
			let converted = crate::error::from_js(error.clone());
			self.thrown.borrow_mut().get_or_insert(error);
			converted
		})
	}
}

/// The `RetryCallbacks` of a JS callbacks object, each called with the object as `this`.
fn retry_callbacks(callbacks: &JsValue, calls: &Calls) -> RetryCallbacks {
	let function = |name: &str| {
		Reflect::get(callbacks, &name.into())
			.ok()
			.and_then(|value| value.dyn_into::<Function>().ok())
	};
	let this = callbacks.clone();
	let on_retry_scheduled = function("onRetryScheduled").map(|f| {
		let (calls, this) = (calls.clone(), this.clone());
		Rc::new(
			move |attempt: f64, max_attempts: f64, delay_ms: f64, error_message: String| {
				let (calls, this, f) = (calls.clone(), this.clone(), f.clone());
				Box::pin(async move {
					let args = [
						attempt.into(),
						max_attempts.into(),
						delay_ms.into(),
						error_message.into(),
					];
					calls.call(&f, &this, &args).await.map(drop)
				}) as LocalBoxFuture<'static, PiResult<()>>
			},
		) as OnRetryScheduled
	});
	let on_retry_attempt_start = function("onRetryAttemptStart").map(|f| {
		let (calls, this) = (calls.clone(), this.clone());
		Rc::new(move || {
			let (calls, this, f) = (calls.clone(), this.clone(), f.clone());
			Box::pin(async move { calls.call(&f, &this, &[]).await.map(drop) }) as LocalBoxFuture<'static, PiResult<()>>
		}) as OnRetryAttemptStart
	});
	let on_retry_finished = function("onRetryFinished").map(|f| {
		let (calls, this) = (calls.clone(), this.clone());
		Rc::new(move |success: bool, attempt: f64, final_error: Option<String>| {
			let (calls, this, f) = (calls.clone(), this.clone(), f.clone());
			Box::pin(async move {
				let mut args = vec![success.into(), attempt.into()];
				if let Some(final_error) = final_error {
					args.push(final_error.into());
				}
				calls.call(&f, &this, &args).await.map(drop)
			}) as LocalBoxFuture<'static, PiResult<()>>
		}) as OnRetryFinished
	});
	RetryCallbacks {
		on_retry_scheduled,
		on_retry_attempt_start,
		on_retry_finished,
	}
}
