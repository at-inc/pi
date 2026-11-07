//! Port of `packages/ai/src/providers/openrouter-api.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `OpenRouterModel.top_provider`'s inline type is named `OpenRouterTopProvider`.

use pi_host::js::parse_float;
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

/// Convert one $/token price to $/1M tokens. 0 means free. Missing, invalid and negative prices use
/// the fallback: OpenRouter reports -1 for router models such as `openrouter/auto`, whose price
/// depends on the model they pick.
fn per_million(value: Option<&str>, fallback: f64) -> f64 {
	let per_token = parse_float(value.unwrap_or(""));
	if per_token >= 0.0 {
		to_fixed_6(per_token * 1_000_000.0)
	} else {
		fallback
	}
}

/// `Number(value.toFixed(6))` for a non-negative `value`. `toFixed` rounds the exact double and takes the larger
/// candidate on a tie, where `format!` rounds ties to even: 0.0078125 is 0.007813, not 0.007812.
fn to_fixed_6(value: f64) -> f64 {
	if value == 0.0 {
		// `(-0).toFixed(6)` is "0.000000".
		return 0.0;
	}
	if !value.is_finite() || value >= 1e21 {
		return value;
	}
	// 1100 places print every double's exact binary value; the smallest subnormal has 1074.
	let exact = format!("{value:.1100}");
	let (integer, fraction) = exact.split_once('.').expect("a fixed-point float has a decimal point");
	let scaled: u128 = format!("{integer}{}", &fraction[..6]).parse().expect("decimal digits");
	let scaled = scaled + u128::from(fraction.as_bytes()[6] >= b'5');
	format!("{}.{:06}", scaled / 1_000_000, scaled % 1_000_000)
		.parse()
		.expect("a fixed-point number")
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
