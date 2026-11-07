//! Rust-side tests of `src/providers/openrouter_api.rs` (`packages/ai/src/providers/openrouter-api.ts`). The TS suite
//! covers `openRouterCost` only through catalog-generation tests, which are not ported; these pin the behavior
//! that the port has to reproduce by hand. Expected values come from running the TS function in node.

use pi_ai::providers::openrouter_api::{OpenRouterPricing, open_router_cost};

fn prompt_cost(prompt: &str) -> f64 {
	open_router_cost(Some(&OpenRouterPricing {
		prompt: Some(prompt.into()),
		..OpenRouterPricing::default()
	}))
	.input
}

mod open_router_cost {
	use super::*;

	#[test]
	fn rounds_an_exact_tie_up_like_to_fixed() {
		// 0.0078125 and 0.0390625 are exact doubles halfway between two 6-digit values.
		assert_eq!(prompt_cost("0.0000000078125"), 0.007813);
		assert_eq!(prompt_cost("0.0000000390625"), 0.039063);
	}

	#[test]
	fn rounds_inexact_values_to_the_nearest_6_digit_value() {
		assert_eq!(prompt_cost("0.000003"), 3.0);
		assert_eq!(prompt_cost("0.0000015"), 1.5);
		assert_eq!(prompt_cost("0.0000000000005"), 0.0);
		assert_eq!(prompt_cost("-0"), 0.0);
		assert!(prompt_cost("-0").is_sign_positive());
	}
}
