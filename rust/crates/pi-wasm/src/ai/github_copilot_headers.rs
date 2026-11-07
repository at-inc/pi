//! Exports of `api/github-copilot-headers`.

use pi_ai::Message;
use pi_ai::api::github_copilot_headers;
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::options::{arg, throw};

/// `inferCopilotInitiator(messages)`: `"user"` or `"agent"`.
#[wasm_bindgen(js_name = aiInferCopilotInitiator)]
pub fn infer_copilot_initiator(messages: JsValue) -> Result<String, JsValue> {
	let messages: Vec<Message> = arg(messages)?;
	Ok(github_copilot_headers::infer_copilot_initiator(&messages)
		.as_str()
		.to_string())
}

/// `hasCopilotVisionInput(messages)`.
#[wasm_bindgen(js_name = aiHasCopilotVisionInput)]
pub fn has_copilot_vision_input(messages: JsValue) -> Result<bool, JsValue> {
	let messages: Vec<Message> = arg(messages)?;
	Ok(github_copilot_headers::has_copilot_vision_input(&messages))
}

/// The `params` of `buildCopilotDynamicHeaders`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DynamicHeadersParams {
	messages: Vec<Message>,
	#[serde(default)]
	has_images: bool,
}

/// `buildCopilotDynamicHeaders({ messages, hasImages })`.
#[wasm_bindgen(js_name = aiBuildCopilotDynamicHeaders)]
pub fn build_copilot_dynamic_headers(params: JsValue) -> Result<JsValue, JsValue> {
	let params: DynamicHeadersParams = arg(params)?;
	let headers = github_copilot_headers::build_copilot_dynamic_headers(&params.messages, params.has_images);
	throw(crate::json::to_js(&headers))
}
