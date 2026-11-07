//! Port of `packages/ai/src/providers/openrouter-api.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `OpenRouterModel.top_provider`'s inline type is named `OpenRouterTopProvider`.
//! - `Number.parseFloat` is a private copy of the one in `utils/provider_retry.rs`, which keeps it private.

use serde::{Deserialize, Serialize};

use crate::types::{ModelCost, ModelCostTier};

/// OpenRouter's per-token prices, as decimal strings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterPricing {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub prompt: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub completion: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_cache_read: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_cache_write: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub overrides: Option<Vec<OpenRouterPricingOverride>>,
}

/// A conditional price. `min_prompt_tokens` selects prompt-length pricing; `utc_*` fields select
/// time-of-day or weekday pricing. Missing rates keep the base price.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterPricingOverride {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub min_prompt_tokens: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub utc_start: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub utc_end: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub utc_days: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub prompt: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub completion: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_cache_read: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub input_cache_write: Option<String>,
}

/// `OpenRouterModel.top_provider`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterTopProvider {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub context_length: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_completion_tokens: Option<f64>,
}

/// The fields pi reads from a model in OpenRouter's `GET /models` and `GET /models/user`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterModel {
	pub id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub context_length: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub top_provider: Option<OpenRouterTopProvider>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub pricing: Option<OpenRouterPricing>,
}

/// `StrWhiteSpaceChar`: JavaScript white space and line terminators.
fn is_js_space(ch: char) -> bool {
	(ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}'
}

/// `Number.parseFloat(text)`: the longest decimal literal at the start of `text` after white space, else NaN.
fn parse_float(text: &str) -> f64 {
	let text = text.trim_start_matches(is_js_space);
	let bytes = text.as_bytes();
	let mut end = 0;
	if matches!(bytes.first(), Some(b'+' | b'-')) {
		end = 1;
	}
	if text[end..].starts_with("Infinity") {
		return if text.starts_with('-') {
			f64::NEG_INFINITY
		} else {
			f64::INFINITY
		};
	}
	let digits = |from: usize| from + bytes[from..].iter().take_while(|byte| byte.is_ascii_digit()).count();
	let integer_end = digits(end);
	let mut mantissa_end = integer_end;
	if bytes.get(integer_end) == Some(&b'.') {
		mantissa_end = digits(integer_end + 1);
		if mantissa_end == integer_end + 1 {
			mantissa_end = if integer_end > end {
				integer_end + 1
			} else {
				integer_end
			};
		}
	}
	if mantissa_end == end || (mantissa_end == end + 1 && bytes[end] == b'.') {
		return f64::NAN;
	}
	let mut literal_end = mantissa_end;
	if matches!(bytes.get(mantissa_end), Some(b'e' | b'E')) {
		let mut exponent_start = mantissa_end + 1;
		if matches!(bytes.get(exponent_start), Some(b'+' | b'-')) {
			exponent_start += 1;
		}
		let exponent_end = digits(exponent_start);
		if exponent_end > exponent_start {
			literal_end = exponent_end;
		}
	}
	text[..literal_end].trim_end_matches('.').parse().unwrap_or(f64::NAN)
}

/// Convert one $/token price to $/1M tokens. 0 means free. Missing, invalid and negative prices use
/// the fallback: OpenRouter reports -1 for router models such as `openrouter/auto`, whose price
/// depends on the model they pick.
fn per_million(value: Option<&str>, fallback: f64) -> f64 {
	let per_token = parse_float(value.unwrap_or(""));
	if per_token >= 0.0 {
		let rounded: f64 = format!("{:.6}", per_token * 1_000_000.0).parse().unwrap_or(f64::NAN);
		// `Number("-0.000000")` is never produced: `toFixed` prints -0 as "0.000000".
		rounded + 0.0
	} else {
		fallback
	}
}

/// Convert OpenRouter's $/token prices to pi's $/1M tokens. Missing, invalid and negative base
/// prices are 0. Prompt-length overrides become request-wide tiers; a rate a tier does not list
/// keeps the base price. Time-of-day overrides are skipped because ModelCost cannot express them.
pub fn open_router_cost(pricing: Option<&OpenRouterPricing>) -> ModelCost {
	let input = per_million(pricing.and_then(|pricing| pricing.prompt.as_deref()), 0.0);
	let output = per_million(pricing.and_then(|pricing| pricing.completion.as_deref()), 0.0);
	let cache_read = per_million(pricing.and_then(|pricing| pricing.input_cache_read.as_deref()), 0.0);
	let cache_write = per_million(pricing.and_then(|pricing| pricing.input_cache_write.as_deref()), 0.0);
	let tiers: Vec<ModelCostTier> = pricing
		.and_then(|pricing| pricing.overrides.as_deref())
		.unwrap_or_default()
		.iter()
		.filter_map(|price_override| {
			let input_tokens_above = price_override.min_prompt_tokens?;
			if price_override.utc_start.is_some()
				|| price_override.utc_end.is_some()
				|| price_override.utc_days.is_some()
			{
				return None;
			}
			Some(ModelCostTier {
				input_tokens_above,
				input: per_million(price_override.prompt.as_deref(), input),
				output: per_million(price_override.completion.as_deref(), output),
				cache_read: per_million(price_override.input_cache_read.as_deref(), cache_read),
				cache_write: per_million(price_override.input_cache_write.as_deref(), cache_write),
			})
		})
		.collect();
	ModelCost {
		input,
		output,
		cache_read,
		cache_write,
		tiers: if tiers.is_empty() { None } else { Some(tiers) },
	}
}
