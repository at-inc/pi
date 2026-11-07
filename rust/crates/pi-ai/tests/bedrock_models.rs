//! Port of `packages/ai/test/bedrock-models.test.ts`.
//!
//! `getModels` (compat.ts) is `get_builtin_models`. Not ported: `should make a simple request with <model id>`,
//! the e2e requests that run only with AWS credentials and `BEDROCK_EXTENSIVE_MODEL_TEST`.

use pi_ai::providers::all::get_builtin_models;

mod amazon_bedrock_models {
	use super::*;

	#[test]
	fn should_get_all_available_bedrock_models() {
		assert!(!get_builtin_models("amazon-bedrock").is_empty());
	}

	#[test]
	fn exposes_claude_opus_5_through_an_inference_profile_only() {
		let models = get_builtin_models("amazon-bedrock");
		assert!(models.iter().any(|model| model.id == "global.anthropic.claude-opus-5"));
		assert!(!models.iter().any(|model| model.id == "anthropic.claude-opus-5"));
	}
}
