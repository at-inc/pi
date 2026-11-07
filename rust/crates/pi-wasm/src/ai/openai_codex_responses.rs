//! Exports of `api/openai-codex-responses` and `api/openai-codex-responses.lazy`.

use std::rc::Rc;

use pi_ai::api::{openai_codex_responses, openai_codex_responses_lazy};
use wasm_bindgen::prelude::*;

use super::models::NativeStreams;
use crate::options::throw;

/// The module's own implementation as `ProviderStreams`, behind its `stream` and `streamSimple` exports.
#[wasm_bindgen(js_name = aiOpenAICodexResponsesStreams)]
pub fn streams() -> NativeStreams {
	NativeStreams::new(Rc::new(openai_codex_responses::OpenAICodexResponsesApi))
}

/// `getOpenAICodexWebSocketDebugStats(sessionId)`: a copy of the counters, `undefined` for an unknown session.
#[wasm_bindgen(js_name = aiGetOpenAICodexWebSocketDebugStats)]
pub fn get_websocket_debug_stats(session_id: &str) -> Result<JsValue, JsValue> {
	match openai_codex_responses::get_openai_codex_websocket_debug_stats(session_id) {
		Some(stats) => throw(crate::json::to_js(&stats)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `resetOpenAICodexWebSocketDebugStats(sessionId)`.
#[wasm_bindgen(js_name = aiResetOpenAICodexWebSocketDebugStats)]
pub fn reset_websocket_debug_stats(session_id: Option<String>) {
	openai_codex_responses::reset_openai_codex_websocket_debug_stats(session_id.as_deref());
}

/// `closeOpenAICodexWebSocketSessions(sessionId)`.
#[wasm_bindgen(js_name = aiCloseOpenAICodexWebSocketSessions)]
pub fn close_websocket_sessions(session_id: Option<String>) {
	openai_codex_responses::close_openai_codex_websocket_sessions(session_id.as_deref());
}

/// `openAICodexResponsesApi()`: the module as lazily loaded `ProviderStreams`.
#[wasm_bindgen(js_name = aiOpenAICodexResponsesApi)]
pub fn api() -> NativeStreams {
	NativeStreams::new(openai_codex_responses_lazy::openai_codex_responses_api())
}
