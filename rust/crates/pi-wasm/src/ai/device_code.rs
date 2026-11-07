//! Exports of `auth/oauth/device-code`.

use std::rc::Rc;

use js_sys::{Function, Promise, Reflect};
use pi_ai::auth::oauth::device_code::{
	OAuthDeviceCodePollOptions, OAuthDeviceCodePollResult, abortable_sleep, poll_oauth_device_code_flow,
};
use pi_host::{AbortController, PiError};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::auth::promise;

/// `pollOAuthDeviceCodeFlow(options)`: `poll` resolves with `{ status: "pending" | "slow_down" | "failed" |
/// "complete", ... }`.
#[wasm_bindgen(js_name = aiPollOAuthDeviceCodeFlow)]
pub fn ai_poll_oauth_device_code_flow(options: JsValue) -> Result<Promise, JsValue> {
	let field = |name: &str| Reflect::get(&options, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let poll: Function = field("poll")
		.dyn_into()
		.map_err(|_| JsValue::from(js_sys::TypeError::new("options.poll is not a function")))?;
	let flow = poll_oauth_device_code_flow(OAuthDeviceCodePollOptions {
		interval_seconds: field("intervalSeconds").as_f64(),
		expires_in_seconds: field("expiresInSeconds").as_f64(),
		wait_before_first_poll: field("waitBeforeFirstPoll").as_bool(),
		poll: Rc::new(move || {
			let called = poll.call0(&JsValue::UNDEFINED).map_err(crate::error::from_js);
			Box::pin(async move {
				let result = JsFuture::from(Promise::resolve(&called?))
					.await
					.map_err(crate::error::from_js)?;
				poll_result(&result)
			})
		}),
		signal: crate::abort::from_js(&field("signal")).unwrap_or_else(|| AbortController::new().signal()),
	});
	Ok(promise(flow))
}

/// The Rust result of a JS poll result.
fn poll_result(result: &JsValue) -> pi_host::PiResult<OAuthDeviceCodePollResult<JsValue>> {
	let field = |name: &str| Reflect::get(result, &name.into()).unwrap_or(JsValue::UNDEFINED);
	match field("status").as_string().as_deref() {
		Some("pending") => Ok(OAuthDeviceCodePollResult::Pending),
		Some("slow_down") => Ok(OAuthDeviceCodePollResult::SlowDown {
			interval_seconds: field("intervalSeconds").as_f64(),
		}),
		Some("failed") => Ok(OAuthDeviceCodePollResult::Failed {
			message: field("message").as_string().unwrap_or_default(),
		}),
		Some("complete") => Ok(OAuthDeviceCodePollResult::Complete { value: field("value") }),
		_ => Err(PiError::type_error("poll result has an unknown status")),
	}
}

/// `abortableSleep(ms, signal, cancelMessage)`.
#[wasm_bindgen(js_name = aiAbortableSleep)]
pub fn ai_abortable_sleep(ms: f64, signal: JsValue, cancel_message: String) -> Promise {
	let signal = crate::abort::from_js(&signal).unwrap_or_else(|| AbortController::new().signal());
	promise(async move {
		abortable_sleep(ms, &signal, &cancel_message).await?;
		Ok(JsValue::UNDEFINED)
	})
}
