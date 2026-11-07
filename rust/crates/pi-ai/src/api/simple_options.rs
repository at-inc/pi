//! Port of `packages/ai/src/api/simple-options.ts`.
//! PORT-STATUS: ported
//!
//! `thinkingBudgetForLevel`'s `{ ...DEFAULT_THINKING_BUDGETS, ...customBudgets }` treats an absent custom budget
//! as not overriding the default; Rust has no key whose value is `undefined`.

use serde_json::Map;

use crate::models::clamp_thinking_level;
use crate::types::{
	Model, ModelThinkingLevel, ProviderRequestOptions, SamplingParams, SimpleStreamOptions, StreamOptions,
	ThinkingBudgets, ThinkingLevel, TranscriptContext,
};
use crate::utils::estimate::estimate_context_tokens;

const CONTEXT_SAFETY_TOKENS: f64 = 4096.0;
const MIN_MAX_TOKENS: f64 = 1.0;

pub fn clamp_max_tokens_to_context(model: &Model, context: &TranscriptContext, max_tokens: f64) -> f64 {
	if model.context_window <= 0.0 {
		return MIN_MAX_TOKENS.max(max_tokens);
	}
	let available = model.context_window - estimate_context_tokens(&context.messages).tokens - CONTEXT_SAFETY_TOKENS;
	max_tokens.min(MIN_MAX_TOKENS.max(available))
}

pub fn resolve_sampling_params(
	model: &Model,
	thinking_level: ModelThinkingLevel,
	request_params: Option<&SamplingParams>,
) -> Option<SamplingParams> {
	let effective_thinking_level = clamp_thinking_level(model, thinking_level);
	let thinking_level_params = model
		.sampling_params_by_thinking_level
		.as_ref()
		.and_then(|params| params.get(&effective_thinking_level));
	if model.sampling_params.is_none() && thinking_level_params.is_none() && request_params.is_none() {
		return None;
	}
	let mut params = Map::new();
	for source in [model.sampling_params.as_ref(), thinking_level_params, request_params]
		.into_iter()
		.flatten()
	{
		for (key, value) in source {
			params.insert(key.clone(), value.clone());
		}
	}
	Some(params)
}

pub fn build_base_options(
	model: &Model,
	context: &TranscriptContext,
	options: Option<&SimpleStreamOptions>,
	api_key: Option<&str>,
) -> StreamOptions {
	let default_options = SimpleStreamOptions::default();
	let options = options.unwrap_or(&default_options);
	let sampling_params = resolve_sampling_params(
		model,
		options
			.reasoning
			.map_or(ModelThinkingLevel::Off, ModelThinkingLevel::from),
		options.sampling_params.as_ref(),
	);
	StreamOptions {
		base: ProviderRequestOptions {
			signal: options.signal.clone(),
			telemetry: options.telemetry.clone(),
			api_key: api_key
				.filter(|api_key| !api_key.is_empty())
				.map(str::to_string)
				.or_else(|| options.api_key.clone()),
			fetch: options.fetch.clone(),
			env: options.env.clone(),
			on_payload: options.on_payload.clone(),
			on_response: options.on_response.clone(),
			headers: options.headers.clone(),
			identity: options.identity.clone(),
			timeout_ms: options.timeout_ms,
			max_retries: options.max_retries,
			max_retry_delay_ms: options.max_retry_delay_ms,
		},
		on_provider_stream_event: options.on_provider_stream_event.clone(),
		temperature: options.temperature,
		sampling_params,
		max_tokens: Some(clamp_max_tokens_to_context(
			model,
			context,
			options.max_tokens.unwrap_or(model.max_tokens),
		)),
		transport: options.transport,
		cache_retention: options.cache_retention,
		session_id: options.session_id.clone(),
		websocket_connect_timeout_ms: options.websocket_connect_timeout_ms,
		metadata: options.metadata.clone(),
	}
}

/// Tokens always left for the answer when a thinking budget shares the response ceiling.
pub const MIN_ANSWER_TOKENS: f64 = 1024.0;

pub const DEFAULT_THINKING_BUDGETS: ThinkingBudgets = ThinkingBudgets {
	minimal: Some(1024.0),
	low: Some(2048.0),
	medium: Some(8192.0),
	high: Some(16384.0),
};

/// `clampReasoning`: `xhigh` and `max` become `high`.
pub fn clamp_reasoning(effort: Option<ThinkingLevel>) -> Option<ThinkingLevel> {
	match effort {
		Some(ThinkingLevel::Xhigh | ThinkingLevel::Max) => Some(ThinkingLevel::High),
		effort => effort,
	}
}

pub fn thinking_budget_for_level(reasoning_level: ThinkingLevel, custom_budgets: Option<&ThinkingBudgets>) -> f64 {
	let custom = custom_budgets.cloned().unwrap_or_default();
	let budgets = ThinkingBudgets {
		minimal: custom.minimal.or(DEFAULT_THINKING_BUDGETS.minimal),
		low: custom.low.or(DEFAULT_THINKING_BUDGETS.low),
		medium: custom.medium.or(DEFAULT_THINKING_BUDGETS.medium),
		high: custom.high.or(DEFAULT_THINKING_BUDGETS.high),
	};
	let budget = match clamp_reasoning(Some(reasoning_level)).expect("a thinking level clamps to a thinking level") {
		ThinkingLevel::Minimal => budgets.minimal,
		ThinkingLevel::Low => budgets.low,
		ThinkingLevel::Medium => budgets.medium,
		ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => budgets.high,
	};
	budget.expect("every clamped thinking level has a default budget")
}

/// Cap a thinking budget so at least MIN_ANSWER_TOKENS remain under a shared response ceiling.
pub fn clamp_thinking_budget_to_answer_room(thinking_budget: f64, ceiling: f64) -> f64 {
	thinking_budget.min(0f64.max(ceiling - MIN_ANSWER_TOKENS))
}

/// `adjustMaxTokensForThinking`'s result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdjustedMaxTokens {
	pub max_tokens: f64,
	pub thinking_budget: f64,
}

pub fn adjust_max_tokens_for_thinking(
	// Undefined means no explicit caller cap. Use the model cap and fit thinking inside it.
	base_max_tokens: Option<f64>,
	model_max_tokens: f64,
	reasoning_level: ThinkingLevel,
	custom_budgets: Option<&ThinkingBudgets>,
) -> AdjustedMaxTokens {
	let mut thinking_budget = thinking_budget_for_level(reasoning_level, custom_budgets);
	let max_tokens = match base_max_tokens {
		None => model_max_tokens,
		Some(base_max_tokens) => (base_max_tokens + thinking_budget).min(model_max_tokens),
	};

	if max_tokens <= thinking_budget {
		thinking_budget = clamp_thinking_budget_to_answer_room(thinking_budget, max_tokens);
	}

	AdjustedMaxTokens {
		max_tokens,
		thinking_budget,
	}
}
