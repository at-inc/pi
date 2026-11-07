//! Exports of `api/simple-options`. Models and contexts cross as JSON text.

use js_sys::{Object, Reflect};
use pi_ai::api::simple_options;
use pi_ai::{Message, Model, ModelThinkingLevel, SamplingParams, ThinkingBudgets, ThinkingLevel, TranscriptContext};
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, to_js_text};
use crate::options::throw;

/// `MIN_ANSWER_TOKENS`.
#[wasm_bindgen(js_name = aiMinAnswerTokens)]
pub fn min_answer_tokens() -> f64 {
	simple_options::MIN_ANSWER_TOKENS
}

/// `DEFAULT_THINKING_BUDGETS`.
#[wasm_bindgen(js_name = aiDefaultThinkingBudgets)]
pub fn default_thinking_budgets() -> Result<JsValue, JsValue> {
	throw(to_js_text(&simple_options::DEFAULT_THINKING_BUDGETS))
}

/// `clampMaxTokensToContext(model, context, maxTokens)`.
#[wasm_bindgen(js_name = aiClampMaxTokensToContext)]
pub fn clamp_max_tokens_to_context(model: JsValue, context: JsValue, max_tokens: f64) -> Result<f64, JsValue> {
	let model: Model = throw(from_js_text(&model))?;
	let messages: Vec<Message> = throw(from_js_text(&Reflect::get(&context, &"messages".into())?))?;
	let context = TranscriptContext::from_normalized_messages(messages);
	Ok(simple_options::clamp_max_tokens_to_context(
		&model, &context, max_tokens,
	))
}

/// `resolveSamplingParams(model, thinkingLevel, requestParams)`.
#[wasm_bindgen(js_name = aiResolveSamplingParams)]
pub fn resolve_sampling_params(
	model: JsValue,
	thinking_level: JsValue,
	request_params: JsValue,
) -> Result<JsValue, JsValue> {
	let model: Model = throw(from_js_text(&model))?;
	let thinking_level: ModelThinkingLevel = throw(from_js_text(&thinking_level))?;
	let request_params: Option<SamplingParams> = throw(from_js_text(&request_params))?;
	match simple_options::resolve_sampling_params(&model, thinking_level, request_params.as_ref()) {
		Some(params) => throw(to_js_text(&params)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `clampReasoning(effort)`.
#[wasm_bindgen(js_name = aiClampReasoning)]
pub fn clamp_reasoning(effort: JsValue) -> Result<JsValue, JsValue> {
	let effort: Option<ThinkingLevel> = throw(from_js_text(&effort))?;
	match simple_options::clamp_reasoning(effort) {
		Some(level) => throw(to_js_text(&level)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `thinkingBudgetForLevel(reasoningLevel, customBudgets)`.
#[wasm_bindgen(js_name = aiThinkingBudgetForLevel)]
pub fn thinking_budget_for_level(reasoning_level: JsValue, custom_budgets: JsValue) -> Result<f64, JsValue> {
	let level: ThinkingLevel = throw(from_js_text(&reasoning_level))?;
	let budgets: Option<ThinkingBudgets> = throw(from_js_text(&custom_budgets))?;
	Ok(simple_options::thinking_budget_for_level(level, budgets.as_ref()))
}

/// `clampThinkingBudgetToAnswerRoom(thinkingBudget, ceiling)`.
#[wasm_bindgen(js_name = aiClampThinkingBudgetToAnswerRoom)]
pub fn clamp_thinking_budget_to_answer_room(thinking_budget: f64, ceiling: f64) -> f64 {
	simple_options::clamp_thinking_budget_to_answer_room(thinking_budget, ceiling)
}

/// `adjustMaxTokensForThinking(baseMaxTokens, modelMaxTokens, reasoningLevel, customBudgets)`.
#[wasm_bindgen(js_name = aiAdjustMaxTokensForThinking)]
pub fn adjust_max_tokens_for_thinking(
	base_max_tokens: Option<f64>,
	model_max_tokens: f64,
	reasoning_level: JsValue,
	custom_budgets: JsValue,
) -> Result<JsValue, JsValue> {
	let level: ThinkingLevel = throw(from_js_text(&reasoning_level))?;
	let budgets: Option<ThinkingBudgets> = throw(from_js_text(&custom_budgets))?;
	let adjusted =
		simple_options::adjust_max_tokens_for_thinking(base_max_tokens, model_max_tokens, level, budgets.as_ref());
	let result = Object::new();
	Reflect::set(&result, &"maxTokens".into(), &adjusted.max_tokens.into())?;
	Reflect::set(&result, &"thinkingBudget".into(), &adjusted.thinking_budget.into())?;
	Ok(result.into())
}
