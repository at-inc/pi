//! Port of `packages/ai/test/env-api-keys.test.ts`.
//!
//! `process.env` is the `TestHost` environment, which starts empty in every test, so the `afterEach` restore and
//! the `delete process.env.X` lines have nothing to do.

use pi_ai::env_api_keys::{find_env_keys, get_env_api_key};

fn strings(values: &[&str]) -> Option<Vec<String>> {
	Some(values.iter().map(|value| value.to_string()).collect())
}

mod environment_api_keys {
	use super::*;

	#[test]
	fn does_not_treat_generic_git_hub_tokens_as_git_hub_copilot_credentials() {
		pi_host::testing::run(|host| async move {
			host.set_env("GH_TOKEN", "gh-token");
			host.set_env("GITHUB_TOKEN", "github-token");

			assert_eq!(find_env_keys("github-copilot", None), None);
			assert_eq!(get_env_api_key("github-copilot", None), None);
		});
	}

	#[test]
	fn resolves_git_hub_copilot_credentials_from_copilot_github_token() {
		pi_host::testing::run(|host| async move {
			host.set_env("COPILOT_GITHUB_TOKEN", "copilot-token");
			host.set_env("GH_TOKEN", "gh-token");
			host.set_env("GITHUB_TOKEN", "github-token");

			assert_eq!(
				find_env_keys("github-copilot", None),
				strings(&["COPILOT_GITHUB_TOKEN"])
			);
			assert_eq!(
				get_env_api_key("github-copilot", None).as_deref(),
				Some("copilot-token")
			);
		});
	}

	#[test]
	fn resolves_zai_china_coding_plan_credentials_from_zai_coding_cn_api_key() {
		pi_host::testing::run(|host| async move {
			host.set_env("ZAI_CODING_CN_API_KEY", "zai-coding-cn-token");

			assert_eq!(
				find_env_keys("zai-coding-cn", None),
				strings(&["ZAI_CODING_CN_API_KEY"])
			);
			assert_eq!(
				get_env_api_key("zai-coding-cn", None).as_deref(),
				Some("zai-coding-cn-token")
			);
		});
	}

	#[test]
	fn reports_anthropic_auth_token_but_preserves_o_auth_token_api_key_lookup() {
		pi_host::testing::run(|host| async move {
			host.set_env("ANTHROPIC_AUTH_TOKEN", "auth-token");
			host.set_env("ANTHROPIC_OAUTH_TOKEN", "oauth-token");
			host.set_env("ANTHROPIC_API_KEY", "api-key");

			assert_eq!(
				find_env_keys("anthropic", None),
				strings(&["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"])
			);
			assert_eq!(get_env_api_key("anthropic", None).as_deref(), Some("oauth-token"));
		});
	}

	#[test]
	fn does_not_return_anthropic_auth_token_as_an_api_key() {
		pi_host::testing::run(|host| async move {
			host.set_env("ANTHROPIC_AUTH_TOKEN", "auth-token");

			assert_eq!(find_env_keys("anthropic", None), strings(&["ANTHROPIC_AUTH_TOKEN"]));
			assert_eq!(get_env_api_key("anthropic", None), None);
		});
	}

	#[test]
	fn preserves_anthropic_oauth_token_as_an_api_key() {
		pi_host::testing::run(|host| async move {
			host.set_env("ANTHROPIC_OAUTH_TOKEN", "oauth-token");

			assert_eq!(find_env_keys("anthropic", None), strings(&["ANTHROPIC_OAUTH_TOKEN"]));
			assert_eq!(get_env_api_key("anthropic", None).as_deref(), Some("oauth-token"));
		});
	}

	#[test]
	fn falls_back_to_anthropic_api_key_for_api_key_lookup() {
		pi_host::testing::run(|host| async move {
			host.set_env("ANTHROPIC_API_KEY", "api-key");

			assert_eq!(get_env_api_key("anthropic", None).as_deref(), Some("api-key"));
		});
	}
}
