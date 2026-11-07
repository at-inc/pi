//! Port of `packages/ai/test/openrouter-cost.test.ts`.

use pi_ai::types::{BaseModel, Usage, UsageCost};
use pi_ai::utils::openrouter_cost::{
	OpenRouterCostDetails, OpenRouterUsageAccounting, apply_open_router_reported_cost, is_open_router_model,
};

fn usage(total: f64) -> Usage {
	Usage {
		input: 1.0,
		output: 1.0,
		cache_read: 0.0,
		cache_write: 0.0,
		total_tokens: 2.0,
		cost: UsageCost {
			input: 0.25,
			output: 0.75,
			cache_read: 0.0,
			cache_write: 0.0,
			total,
			..UsageCost::default()
		},
		..Usage::default()
	}
}

fn model(provider: &str, base_url: &str) -> BaseModel {
	BaseModel {
		provider: provider.into(),
		base_url: base_url.into(),
		..BaseModel::default()
	}
}

fn details(upstream: Option<f64>, prompt: f64, completions: f64) -> OpenRouterCostDetails {
	OpenRouterCostDetails {
		upstream_inference_cost: Some(upstream),
		upstream_inference_prompt_cost: Some(prompt),
		upstream_inference_completions_cost: Some(completions),
		server_tool_cost: None,
	}
}

mod is_open_router_model {
	use super::*;

	#[test]
	fn recognizes_both_the_built_in_provider_and_custom_providers_using_open_routers_endpoint() {
		assert!(is_open_router_model(&model("openrouter", "https://example.com")));
		assert!(is_open_router_model(&model(
			"openrouter-work",
			"https://openrouter.ai/api/v1"
		)));
		assert!(!is_open_router_model(&model("custom", "https://example.com/v1")));
	}
}

mod apply_open_router_reported_cost {
	use super::*;

	#[test]
	fn uses_open_routers_reported_cost_for_non_byok_requests() {
		let mut value = usage(1.0);
		apply_open_router_reported_cost(
			&mut value,
			&OpenRouterUsageAccounting {
				cost: Some(0.0),
				is_byok: Some(false),
				cost_details: Some(details(Some(0.0), 0.0, 0.0)),
			},
		);
		assert_eq!(value.cost.total, 0.0);
		assert_eq!(value.cost.total_from_provider, Some(true));
	}

	#[test]
	fn adds_the_upstream_inference_cost_for_byok_requests() {
		let mut value = usage(1.0);
		apply_open_router_reported_cost(
			&mut value,
			&OpenRouterUsageAccounting {
				cost: Some(0.1),
				is_byok: Some(true),
				cost_details: Some(details(Some(0.9), 0.6, 0.3)),
			},
		);
		assert_eq!(value.cost.total, 1.0);
		assert_eq!(value.cost.total_from_provider, Some(true));
	}

	#[test]
	fn uses_upstream_cost_components_when_open_router_omits_their_total() {
		let mut value = usage(1.0);
		apply_open_router_reported_cost(
			&mut value,
			&OpenRouterUsageAccounting {
				cost: Some(0.1),
				is_byok: Some(true),
				cost_details: Some(details(None, 0.6, 0.3)),
			},
		);
		assert!((value.cost.total - 1.0).abs() < 0.005);
		assert_eq!(value.cost.total_from_provider, Some(true));
	}

	#[test]
	fn keeps_the_catalog_total_when_open_routers_accounting_is_incomplete() {
		let mut value = usage(1.0);
		value.cost.total_from_provider = Some(false);
		apply_open_router_reported_cost(
			&mut value,
			&OpenRouterUsageAccounting {
				cost: None,
				is_byok: Some(false),
				cost_details: None,
			},
		);
		apply_open_router_reported_cost(
			&mut value,
			&OpenRouterUsageAccounting {
				cost: Some(0.1),
				is_byok: Some(true),
				cost_details: None,
			},
		);
		assert_eq!(value.cost.total, 1.0);
		assert_eq!(value.cost.total_from_provider, Some(false));
	}

	#[test]
	fn reads_the_accounting_of_an_open_router_usage_object() {
		let reported: OpenRouterUsageAccounting = serde_json::from_value(serde_json::json!({
			"cost": 0.1,
			"is_byok": true,
			"cost_details": {
				"upstream_inference_cost": null,
				"upstream_inference_prompt_cost": 0.6,
				"upstream_inference_completions_cost": 0.3,
			},
		}))
		.unwrap();
		assert_eq!(reported.cost_details.unwrap().upstream_inference_cost, Some(None));
	}
}
