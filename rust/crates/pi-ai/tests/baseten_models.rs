//! Port of `packages/ai/test/baseten-models.test.ts`.
//!
//! `getModel` (compat.ts) is `get_builtin_model`. Not ported: `sends Baseten chat_template_args with reasoning
//! effort` and `disables Baseten opt-in reasoning when thinking is off`, which capture the request through
//! `streamSimple` on the openai-completions API (wave-3, `w3-openai-completions`). `process.env` is the `TestHost`
//! environment, so the `afterEach` restore is not needed.

use pi_ai::env_api_keys::{find_env_keys, get_env_api_key};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::ModelInputModality;

mod baseten_models {
	use super::*;

	#[test]
	fn keeps_both_glm_5_2_endpoints_text_only() {
		for id in ["zai-org/GLM-5.2", "zai-org/GLM-5.2-Fast"] {
			assert_eq!(
				get_builtin_model("baseten", id).unwrap().input,
				[ModelInputModality::Text],
				"{id}"
			);
		}
	}

	#[test]
	fn resolves_baseten_api_key_from_the_environment() {
		pi_host::testing::run(|host| async move {
			host.set_env("BASETEN_API_KEY", "test-baseten-key");

			assert_eq!(
				find_env_keys("baseten", None),
				Some(vec!["BASETEN_API_KEY".to_string()])
			);
			assert_eq!(get_env_api_key("baseten", None).as_deref(), Some("test-baseten-key"));
		});
	}
}
