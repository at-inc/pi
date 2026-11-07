//! Port of `packages/ai/test/kimi-coding-endpoints.test.ts`.
//!
//! `it.each` cases run in a loop.

use indexmap::IndexMap;
use pi_ai::auth::oauth::kimi_coding::kimi_coding_oauth;
use pi_ai::auth::types::{ModelAuth, OAuthCredential};
use pi_ai::utils::kimi_coding::{KimiCodingEndpoints, KimiCodingEndpointsInput, kimi_coding_endpoints};
use serde_json::json;

// #10720
mod kimi_chat_completions_endpoints {
	use super::*;

	#[test]
	fn normalizes_the_inference_suffix_once() {
		for suffix in ["", "/", "/v1", "/v1/", "/v1/v1/"] {
			assert_eq!(
				kimi_coding_endpoints(KimiCodingEndpointsInput {
					oauth_host: None,
					base_url: Some(format!("https://api.kimi.ai/coding{suffix}")),
				})
				.unwrap(),
				KimiCodingEndpoints {
					oauth_host: "https://auth.kimi.ai".into(),
					base_url: "https://api.kimi.ai/coding/v1".into(),
				},
				"suffix {suffix:?}"
			);
		}
	}

	#[test]
	fn uses_the_stored_deployment_for_oauth_inference() {
		for domain in ["ai", "com"] {
			let mut credential = OAuthCredential::new("test-refresh", "test-access", 0.0);
			credential
				.extra
				.insert("oauthHost".into(), json!(format!("https://auth.kimi.{domain}")));
			credential
				.extra
				.insert("baseUrl".into(), json!(format!("https://api.kimi.{domain}/coding")));
			let auth = pi_host::testing::run(|_| (kimi_coding_oauth().to_auth)(credential)).unwrap();

			assert_eq!(
				auth,
				ModelAuth {
					api_key: None,
					headers: Some(IndexMap::from([(
						"Authorization".to_string(),
						Some("Bearer test-access".to_string())
					)])),
					base_url: Some(format!("https://api.kimi.{domain}/coding/v1")),
				},
				"{domain}"
			);
		}
	}

	#[test]
	fn does_not_infer_the_international_deployment_from_a_lookalike_hostname() {
		assert_eq!(
			kimi_coding_endpoints(KimiCodingEndpointsInput {
				oauth_host: None,
				base_url: Some("https://api.kimi.ai.example/coding".into()),
			})
			.unwrap(),
			KimiCodingEndpoints {
				oauth_host: "https://auth.kimi.com".into(),
				base_url: "https://api.kimi.ai.example/coding/v1".into(),
			}
		);
	}
}
