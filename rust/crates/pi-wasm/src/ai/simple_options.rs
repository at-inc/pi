//! Exports of `api/simple-options`.

use pi_ai::api::simple_options;
use pi_ai::{ModelThinkingLevel, SamplingParams, ThinkingBudgets, ThinkingLevel};
use serde_json::{Map, json};
use wasm_bindgen::prelude::*;

use crate::options::{arg, simple_stream_options, throw};

/// `clampMaxTokensToContext(model, context, maxTokens)`.
#[wasm_bindgen(js_name = aiClampMaxTokensToContext)]
pub fn clamp_max_tokens_to_context(model: JsValue, context: JsValue, max_tokens: f64) -> Result<f64, JsValue> {
	let model = super::model(&model)?;
	let context = super::transcript(context)?;
	Ok(simple_options::clamp_max_tokens_to_context(
		&model, &context, max_tokens,
	))
}

/// `resolveSamplingParams(model, thinkingLevel, requestParams)`: `undefined` when no source has params.
#[wasm_bindgen(js_name = aiResolveSamplingParams)]
pub fn resolve_sampling_params(
	model: JsValue,
	thinking_level: JsValue,
	request_params: JsValue,
) -> Result<JsValue, JsValue> {
	let model = super::model(&model)?;
	let thinking_level: ModelThinkingLevel = arg(thinking_level)?;
	let request_params: Option<SamplingParams> = arg(request_params)?;
	match simple_options::resolve_sampling_params(&model, thinking_level, request_params.as_ref()) {
		Some(params) => throw(crate::json::to_js(&params)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// The plain data fields of `buildBaseOptions(model, context, options, apiKey)`. The fields that hold objects with
/// identity (`signal`, `telemetry`, `fetch` and the callbacks) are the caller's own, so the facade copies them from
/// `options` instead of sending them through Rust.
#[wasm_bindgen(js_name = aiBuildBaseOptions)]
pub fn build_base_options(
	model: JsValue,
	context: JsValue,
	options: JsValue,
	api_key: JsValue,
) -> Result<JsValue, JsValue> {
	let parsed = super::model(&model)?;
	let context = super::transcript(context)?;
	let options = simple_stream_options(options, &model);
	let api_key: Option<String> = arg(api_key)?;
	let mut base = simple_options::build_base_options(&parsed, &context, options.as_ref(), api_key.as_deref());
	base.base.signal = None;
	base.base.fetch = None;
	base.base.on_payload = None;
	base.base.on_response = None;
	base.on_provider_stream_event = None;
	let (object, _) = throw(crate::options::stream_options_to_js(&base, &Map::new()))?;
	Ok(object)
}

/// `MIN_ANSWER_TOKENS`.
#[wasm_bindgen(js_name = aiMinAnswerTokens)]
pub fn min_answer_tokens() -> f64 {
	simple_options::MIN_ANSWER_TOKENS
}

/// `DEFAULT_THINKING_BUDGETS`.
#[wasm_bindgen(js_name = aiDefaultThinkingBudgets)]
pub fn default_thinking_budgets() -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&simple_options::DEFAULT_THINKING_BUDGETS))
}

/// `clampReasoning(effort)`: `xhigh` and `max` become `high`.
#[wasm_bindgen(js_name = aiClampReasoning)]
pub fn clamp_reasoning(effort: JsValue) -> Result<JsValue, JsValue> {
	let effort: Option<ThinkingLevel> = arg(effort)?;
	match simple_options::clamp_reasoning(effort) {
		Some(level) => throw(crate::json::to_js(&level)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `thinkingBudgetForLevel(reasoningLevel, customBudgets)`.
#[wasm_bindgen(js_name = aiThinkingBudgetForLevel)]
pub fn thinking_budget_for_level(reasoning_level: JsValue, custom_budgets: JsValue) -> Result<f64, JsValue> {
	let reasoning_level: ThinkingLevel = arg(reasoning_level)?;
	let custom_budgets: Option<ThinkingBudgets> = arg(custom_budgets)?;
	Ok(simple_options::thinking_budget_for_level(
		reasoning_level,
		custom_budgets.as_ref(),
	))
}

/// `clampThinkingBudgetToAnswerRoom(thinkingBudget, ceiling)`.
#[wasm_bindgen(js_name = aiClampThinkingBudgetToAnswerRoom)]
pub fn clamp_thinking_budget_to_answer_room(thinking_budget: f64, ceiling: f64) -> f64 {
	simple_options::clamp_thinking_budget_to_answer_room(thinking_budget, ceiling)
}

/// `adjustMaxTokensForThinking(baseMaxTokens, modelMaxTokens, reasoningLevel, customBudgets)`:
/// `{ maxTokens, thinkingBudget }`.
#[wasm_bindgen(js_name = aiAdjustMaxTokensForThinking)]
pub fn adjust_max_tokens_for_thinking(
	base_max_tokens: Option<f64>,
	model_max_tokens: f64,
	reasoning_level: JsValue,
	custom_budgets: JsValue,
) -> Result<JsValue, JsValue> {
	let reasoning_level: ThinkingLevel = arg(reasoning_level)?;
	let custom_budgets: Option<ThinkingBudgets> = arg(custom_budgets)?;
	let adjusted = simple_options::adjust_max_tokens_for_thinking(
		base_max_tokens,
		model_max_tokens,
		reasoning_level,
		custom_budgets.as_ref(),
	);
	throw(crate::json::to_js(&json!({
		"maxTokens": adjusted.max_tokens,
		"thinkingBudget": adjusted.thinking_budget,
	})))
}
