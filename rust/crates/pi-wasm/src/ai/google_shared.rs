//! Exports of `api/google-shared`.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Function, Reflect};
use pi_ai::api::google_shared::{self, GoogleApiThinkingLevel, GoogleRetryOptions, ResolvedGoogleThinkingLevel};
use pi_ai::{ModelThinkingLevel, Tool};
use pi_host::PiError;
use pi_host::js::json_stringify;
use serde_json::Value;
use wasm_bindgen::prelude::*;

use crate::options::{arg, throw};

/// `resolveGoogleThinkingLevel(model, level)`.
#[wasm_bindgen(js_name = aiGoogleSharedResolveGoogleThinkingLevel)]
pub fn resolve_google_thinking_level(model: JsValue, level: JsValue) -> Result<JsValue, JsValue> {
	let model = super::model(&model)?;
	let level = throw(google_shared::resolve_google_thinking_level(
		&model,
		arg::<ModelThinkingLevel>(level)?,
	))?;
	Ok(level.as_str().into())
}

/// `usesGoogleThinkingLevel(model)`.
#[wasm_bindgen(js_name = aiGoogleSharedUsesGoogleThinkingLevel)]
pub fn uses_google_thinking_level(model: JsValue) -> Result<bool, JsValue> {
	Ok(google_shared::uses_google_thinking_level(&super::model(&model)?))
}

/// `toGoogleThinkingLevel(level)`: `undefined` for a level outside the switch, as in the TS.
#[wasm_bindgen(js_name = aiGoogleSharedToGoogleThinkingLevel)]
pub fn to_google_thinking_level(level: &str) -> JsValue {
	let level = match level {
		"minimal" => ResolvedGoogleThinkingLevel::Minimal,
		"low" => ResolvedGoogleThinkingLevel::Low,
		"medium" => ResolvedGoogleThinkingLevel::Medium,
		"high" => ResolvedGoogleThinkingLevel::High,
		_ => return JsValue::UNDEFINED,
	};
	google_shared::to_google_thinking_level(level).as_str().into()
}

/// `toGoogleSdkThinkingLevel(level)`: `undefined` for a level outside the map, as in the TS.
#[wasm_bindgen(js_name = aiGoogleSharedToGoogleSdkThinkingLevel)]
pub fn to_google_sdk_thinking_level(level: &str) -> JsValue {
	let level = match level {
		"THINKING_LEVEL_UNSPECIFIED" => GoogleApiThinkingLevel::ThinkingLevelUnspecified,
		"MINIMAL" => GoogleApiThinkingLevel::Minimal,
		"LOW" => GoogleApiThinkingLevel::Low,
		"MEDIUM" => GoogleApiThinkingLevel::Medium,
		"HIGH" => GoogleApiThinkingLevel::High,
		_ => return JsValue::UNDEFINED,
	};
	google_shared::to_google_sdk_thinking_level(level).as_str().into()
}

/// `getDisabledGoogleThinkingConfig(model)`.
#[wasm_bindgen(js_name = aiGoogleSharedGetDisabledGoogleThinkingConfig)]
pub fn get_disabled_google_thinking_config(model: JsValue) -> Result<JsValue, JsValue> {
	let config = throw(google_shared::get_disabled_google_thinking_config(&super::model(
		&model,
	)?))?;
	throw(crate::json::to_js(&config))
}

/// `isThinkingPart(part)`.
#[wasm_bindgen(js_name = aiGoogleSharedIsThinkingPart)]
pub fn is_thinking_part(part: JsValue) -> Result<bool, JsValue> {
	Ok(google_shared::is_thinking_part(&arg::<Value>(part)?))
}

/// `retainThoughtSignature(existing, incoming)`.
#[wasm_bindgen(js_name = aiGoogleSharedRetainThoughtSignature)]
pub fn retain_thought_signature(existing: Option<String>, incoming: Option<String>) -> Option<String> {
	google_shared::retain_thought_signature(existing, incoming.as_deref())
}

/// `requiresToolCallId(modelId)`.
#[wasm_bindgen(js_name = aiGoogleSharedRequiresToolCallId)]
pub fn requires_tool_call_id(model_id: &str) -> bool {
	google_shared::requires_tool_call_id(model_id)
}

/// `convertMessages(model, context)`.
#[wasm_bindgen(js_name = aiGoogleSharedConvertMessages)]
pub fn convert_messages(model: JsValue, context: JsValue) -> Result<JsValue, JsValue> {
	let model = super::model(&model)?;
	let context = super::transcript(context)?;
	throw(crate::json::to_js(&google_shared::convert_messages(&model, &context)))
}

/// `convertTools(tools, useParameters, supportsStrictMode)`: `undefined` for no tools. The declarations cross through
/// `JSON.parse`, which keeps a schema name such as `__proto__` as an own property, as the TS `Object.fromEntries` does;
/// the serde serializer assigns keys, which would set the object's prototype instead.
#[wasm_bindgen(js_name = aiGoogleSharedConvertTools)]
pub fn convert_tools(
	tools: JsValue,
	use_parameters: Option<bool>,
	supports_strict_mode: Option<bool>,
) -> Result<JsValue, JsValue> {
	let tools: Vec<Tool> = arg(tools)?;
	match throw(google_shared::convert_tools(
		&tools,
		use_parameters,
		supports_strict_mode,
	))? {
		Some(declarations) => js_sys::JSON::parse(&json_stringify(&Value::Array(declarations))),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `supportsGoogleStrictToolSampling(modelId)`.
#[wasm_bindgen(js_name = aiGoogleSharedSupportsGoogleStrictToolSampling)]
pub fn supports_google_strict_tool_sampling(model_id: &str) -> bool {
	google_shared::supports_google_strict_tool_sampling(model_id)
}

/// `mapToolChoice(choice)`.
#[wasm_bindgen(js_name = aiGoogleSharedMapToolChoice)]
pub fn map_tool_choice(choice: &str) -> String {
	google_shared::map_tool_choice(choice).as_str().into()
}

/// `resolveGoogleFunctionCallingMode(tools, toolChoice, supportsStrictMode)`: `undefined` when no mode applies.
#[wasm_bindgen(js_name = aiGoogleSharedResolveGoogleFunctionCallingMode)]
pub fn resolve_google_function_calling_mode(
	tools: JsValue,
	tool_choice: Option<String>,
	supports_strict_mode: bool,
) -> Result<Option<String>, JsValue> {
	let tools: Vec<Tool> = arg(tools)?;
	let mode = throw(google_shared::resolve_google_function_calling_mode(
		&tools,
		tool_choice.as_deref(),
		supports_strict_mode,
	))?;
	Ok(mode.map(|mode| mode.as_str().into()))
}

/// `mapStopReason(reason)`.
#[wasm_bindgen(js_name = aiGoogleSharedMapStopReason)]
pub fn map_stop_reason(reason: JsValue) -> Result<JsValue, JsValue> {
	let reason = reason
		.as_string()
		.unwrap_or_else(|| String::from(js_sys::JsString::from(reason)));
	let stop_reason = throw(google_shared::map_stop_reason(&reason))?;
	throw(crate::json::to_js(&stop_reason))
}

/// `mapStopReasonString(reason)`.
#[wasm_bindgen(js_name = aiGoogleSharedMapStopReasonString)]
pub fn map_stop_reason_string(reason: &str) -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&google_shared::map_stop_reason_string(reason)))
}

/// `retryGoogleRequest(request, { maxRetries, maxRetryDelayMs, signal })`: a Promise of the request's result. A
/// request's error rejects as the thrown JS value itself; like the TS, an `Error` with a `status` and no `headers`
/// gets `headers: undefined` before the retry policy sees it.
#[wasm_bindgen(js_name = aiGoogleSharedRetryGoogleRequest)]
pub fn retry_google_request(request: Function, options: JsValue) -> js_sys::Promise {
	let field = |name: &str| Reflect::get(&options, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let retry_options = GoogleRetryOptions {
		max_retries: field("maxRetries").as_f64(),
		max_retry_delay_ms: field("maxRetryDelayMs").as_f64(),
		signal: crate::abort::from_js(&field("signal")),
	};
	let last_error: Rc<RefCell<Option<(JsValue, PiError)>>> = Rc::default();
	let attempt_error = last_error.clone();
	wasm_bindgen_futures::future_to_promise(async move {
		let result = google_shared::retry_google_request(
			|| {
				let (request, attempt_error) = (request.clone(), attempt_error.clone());
				async move {
					let args = js_sys::Array::new();
					let thrown = match request.apply(&JsValue::UNDEFINED, &args) {
						Ok(result) => {
							match wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&result)).await {
								Ok(value) => return Ok(value),
								Err(thrown) => thrown,
							}
						}
						Err(thrown) => thrown,
					};
					if thrown.is_instance_of::<js_sys::Error>()
						&& Reflect::has(&thrown, &"status".into()).unwrap_or(false)
						&& !Reflect::has(&thrown, &"headers".into()).unwrap_or(true)
					{
						let _ = Reflect::set(&thrown, &"headers".into(), &JsValue::UNDEFINED);
					}
					let error = crate::error::from_js(thrown.clone());
					*attempt_error.borrow_mut() = Some((thrown, error.clone()));
					Err(error)
				}
			},
			Some(retry_options),
		)
		.await;
		result.map_err(|error| match last_error.borrow_mut().take() {
			Some((thrown, last)) if last.name == error.name && last.message == error.message => thrown,
			_ => crate::error::to_js(&error),
		})
	})
}
