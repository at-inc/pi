//! Port of `packages/ai/src/utils/openrouter-cost.ts`.
//! PORT-STATUS: ported
//!
//! `OpenRouterUsageAccounting` deserializes the `usage` object of an OpenRouter response. Every field may be absent
//! (TS reads it as `undefined`, which none of the checks accept), and `upstream_inference_cost` keeps absent apart
//! from `null`, because only `null` falls back to the cost components. A field of another JSON type fails
//! deserialization, where TS would read the value and skip it. `isOpenRouterModel` takes the shared model fields.

use serde::{Deserialize, Serialize};

use crate::types::{BaseModel, Usage};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterUsageAccounting {
	/// Amount charged to the user's OpenRouter account.
	#[serde(default)]
	pub cost: Option<f64>,
	/// Whether OpenRouter used the user's upstream provider key.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub is_byok: Option<bool>,
	#[serde(default)]
	pub cost_details: Option<OpenRouterCostDetails>,
}

/// `OpenRouterUsageAccounting.cost_details`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterCostDetails {
	/// Total inference cost billed through the user's upstream provider account, when available.
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub upstream_inference_cost: Option<Option<f64>>,
	/// Upstream cost for prompt processing, including any provider-side cache pricing.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub upstream_inference_prompt_cost: Option<f64>,
	/// Upstream cost for completion generation.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub upstream_inference_completions_cost: Option<f64>,
	/// Metered server-tool cost already included in OpenRouter's charged `cost`.
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub server_tool_cost: Option<Option<f64>>,
}

fn is_valid_cost(value: Option<f64>) -> Option<f64> {
	value.filter(|value| value.is_finite() && *value >= 0.0)
}

pub fn is_open_router_model<A>(model: &BaseModel<A>) -> bool {
	model.provider == "openrouter" || model.base_url.contains("openrouter.ai")
}

/// Replace the catalog total with OpenRouter's billed cost when the response contains complete, valid accounting.
pub fn apply_open_router_reported_cost(usage: &mut Usage, reported: &OpenRouterUsageAccounting) {
	let Some(cost) = is_valid_cost(reported.cost) else {
		return;
	};

	if reported.is_byok == Some(false) {
		usage.cost.total = cost;
		usage.cost.total_from_provider = Some(true);
		return;
	}

	if reported.is_byok == Some(true) {
		let Some(details) = &reported.cost_details else {
			return;
		};
		let upstream_cost = match details.upstream_inference_cost {
			Some(None) => {
				let (Some(prompt_cost), Some(completions_cost)) = (
					is_valid_cost(details.upstream_inference_prompt_cost),
					is_valid_cost(details.upstream_inference_completions_cost),
				) else {
					return;
				};
				Some(prompt_cost + completions_cost)
			}
			Some(Some(upstream_cost)) => Some(upstream_cost),
			None => None,
		};
		let Some(upstream_cost) = is_valid_cost(upstream_cost) else {
			return;
		};
		usage.cost.total = cost + upstream_cost;
		usage.cost.total_from_provider = Some(true);
	}
}
