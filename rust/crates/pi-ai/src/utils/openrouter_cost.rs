//! Port of `packages/ai/src/utils/openrouter-cost.ts`.
//! PORT-STATUS: ported
//!
//! TS casts the raw OpenRouter `usage` object to `OpenRouterUsageAccounting` and checks each field at runtime, so
//! the reported accounting is the raw JSON value here: a field that is absent or of another type fails the same
//! checks it fails in TS, and only `upstream_inference_cost: null` falls back to the cost components.
//! `isOpenRouterModel` takes the shared model fields.

use serde_json::Value;

use crate::types::{BaseModel, Usage};

/// `isValidCost`: a finite, non-negative number.
fn valid_cost(value: Option<&Value>) -> Option<f64> {
	value
		.and_then(Value::as_f64)
		.filter(|value| value.is_finite() && *value >= 0.0)
}

pub fn is_open_router_model<A>(model: &BaseModel<A>) -> bool {
	model.provider == "openrouter" || model.base_url.contains("openrouter.ai")
}

/// Replace the catalog total with OpenRouter's billed cost when the response contains complete, valid accounting.
pub fn apply_open_router_reported_cost(usage: &mut Usage, reported: &Value) {
	let Some(cost) = valid_cost(reported.get("cost")) else {
		return;
	};

	match reported.get("is_byok") {
		Some(Value::Bool(false)) => {
			usage.cost.total = cost;
			usage.cost.total_from_provider = Some(true);
		}
		Some(Value::Bool(true)) => {
			let Some(details) = reported.get("cost_details").and_then(Value::as_object) else {
				return;
			};
			let upstream_cost = match details.get("upstream_inference_cost") {
				Some(Value::Null) => {
					let (Some(prompt_cost), Some(completions_cost)) = (
						valid_cost(details.get("upstream_inference_prompt_cost")),
						valid_cost(details.get("upstream_inference_completions_cost")),
					) else {
						return;
					};
					Some(prompt_cost + completions_cost)
				}
				upstream_cost => valid_cost(upstream_cost),
			};
			let Some(upstream_cost) = upstream_cost.filter(|value| value.is_finite() && *value >= 0.0) else {
				return;
			};
			usage.cost.total = cost + upstream_cost;
			usage.cost.total_from_provider = Some(true);
		}
		_ => {}
	}
}
