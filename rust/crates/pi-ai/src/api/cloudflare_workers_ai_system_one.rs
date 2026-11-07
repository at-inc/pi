//! Port of `packages/ai/src/api/cloudflare-workers-ai-system-one.ts`.
//! PORT-STATUS: reviewed
//!
//! `String(result.state)` is ported for JSON values: `undefined` for a missing key, otherwise the JS string
//! conversion of the value.

use pi_host::js::number_to_string;
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::{Map, Value};

use super::system_one_shared::{
	ResolvedSystemOneCompat, SystemOneTransport, SystemOneWireRequest, classify_system_one, join_base_url,
};
use crate::providers::compat_schema::SystemOneImageInput;
use crate::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult};

const LABEL: &str = "Cloudflare Workers AI";

fn cloudflare_error_message(errors: Option<&Value>) -> String {
	if let Some(Value::Array(errors)) = errors {
		let messages: Vec<&str> = errors
			.iter()
			.filter_map(|error| error.get("message").and_then(Value::as_str))
			.collect();
		if !messages.is_empty() {
			return format!("{LABEL} error: {}", messages.join("; "));
		}
	}
	format!("{LABEL} request failed")
}

/// System One models on the Workers AI REST endpoint:
/// `POST /accounts/{account}/ai/run` with `{ model, input }`. The REST API wraps the model
/// output in Cloudflare's API envelope. Third-party models such as `typesafe/jev` add a run record:
/// `{ success, result: { state: "Completed", result: { answers, usage } } }`.
/// https://developers.cloudflare.com/ai/models/typesafe/jev/
/// Cloudflare-hosted models such as `@cf/cloudflare/clef` return the output directly:
/// `{ success, result: { model, answers, usage } }`.
/// https://developers.cloudflare.com/workers-ai/models/clef/
///
/// Clef models take images as an `images` list of data URLs. TypeSafe's models take them as
/// System One `assets`, without TypeSafe's opt-in header. The catalog sets `compat.imageInput`;
/// the model ID check only covers models without it.
static TRANSPORT: SystemOneTransport = SystemOneTransport {
	api: "cloudflare-workers-ai-system-one",
	label: LABEL,
	detect_compat: |model| ResolvedSystemOneCompat {
		image_input: if model.id.starts_with("@cf/cloudflare/clef") {
			SystemOneImageInput::Images
		} else {
			SystemOneImageInput::Assets
		},
		send_image_opt_in_header: false,
	},
	url: |model| join_base_url(&model.base_url, "run"),
	payload,
	output,
};

/// `{ model: model.id, input: request }`.
fn payload(model: &ClassifierModel, request: SystemOneWireRequest) -> Value {
	let mut payload = Map::new();
	payload.insert("model".into(), Value::String(model.id.clone()));
	payload.insert("input".into(), Value::Object(request));
	Value::Object(payload)
}

fn unexpected_response() -> PiError {
	PiError::new(format!("{LABEL} returned an unexpected response"))
}

fn output(body: Value) -> PiResult<Map<String, Value>> {
	let Value::Object(mut body) = body else {
		return Err(unexpected_response());
	};
	if body.get("success") == Some(&Value::Bool(false)) {
		return Err(PiError::new(cloudflare_error_message(body.get("errors"))));
	}
	let Some(Value::Object(mut result)) = body.remove("result") else {
		return Err(unexpected_response());
	};
	if result.contains_key("answers") {
		return Ok(result);
	}
	if result.get("state").and_then(Value::as_str) != Some("Completed") {
		return Err(PiError::new(format!(
			"{LABEL} run did not complete (state: {})",
			js_string(result.get("state"))
		)));
	}
	match result.remove("result") {
		Some(Value::Object(output)) => Ok(output),
		_ => Err(unexpected_response()),
	}
}

/// `String(value)` for a JSON value; `None` is `undefined`.
fn js_string(value: Option<&Value>) -> String {
	match value {
		None => "undefined".into(),
		Some(Value::Null) => "null".into(),
		Some(Value::Bool(value)) => value.to_string(),
		Some(Value::Number(number)) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Some(Value::String(text)) => text.clone(),
		Some(Value::Array(items)) => items
			.iter()
			.map(|item| match item {
				Value::Null => String::new(),
				item => js_string(Some(item)),
			})
			.collect::<Vec<_>>()
			.join(","),
		Some(Value::Object(_)) => "[object Object]".into(),
	}
}

/// Cloudflare Workers AI System One classification with public `bool` values mapped to wire-level `noul`.
pub fn classify(
	model: &ClassifierModel,
	context: &ClassifierContext,
	options: Option<ClassifierOptions>,
) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
	classify_system_one(&TRANSPORT, model, context, options)
}
