//! Port of `packages/ai/test/oauth-auth.test.ts`.
//!
//! The fetch-stubbing tests run on `pi_host::testing`. Not ported here: "keeps the extension OAuth barrel free of
//! built-in flow implementations" (in Rust `oauth.rs` re-exports only the extension types, which the compiler
//! enforces, so there is no runtime property to inspect), and the cases that go through `openaiProvider()`,
//! `anthropicProvider()`, `githubCopilotProvider()` and `createModels` ("OpenAI exposes ChatGPT OAuth alongside
//! API-key auth" and the "OAuth through Models.getAuth (lazy load chain)" group), which exercise `models.ts` and the
//! providers of shard w2-ai-core.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::json_response;
use pi_ai::auth::oauth::anthropic::anthropic_oauth;
use pi_ai::auth::oauth::github_copilot::github_copilot_oauth;
use pi_ai::auth::oauth::kimi_coding::kimi_coding_oauth;
use pi_ai::auth::oauth::openai_chatgpt::openai_chat_gpt_oauth;
use pi_ai::auth::oauth::openai_codex::openai_codex_oauth;
use pi_ai::auth::oauth::openrouter::open_router_oauth;
use pi_ai::auth::oauth::xai::xai_oauth;
use pi_ai::auth::types::{ModelAuth, OAuthCredential};
use pi_host::AbortController;
use serde_json::{Value, json};

fn credential(access: &str, refresh: &str, expires: f64) -> OAuthCredential {
	OAuthCredential::new(refresh, access, expires)
}

fn auth_json(auth: &ModelAuth) -> Value {
	serde_json::to_value(auth).unwrap()
}

fn to_auth(oauth: pi_ai::auth::types::OAuthAuth, credential: OAuthCredential) -> ModelAuth {
	common::run_with_fake_timers(Rc::new(|_| unreachable!("toAuth does not fetch")), || {
		oauth.to_auth(credential)
	})
	.unwrap()
}

mod o_auth_auth_adapters {
	use super::*;

	#[test]
	fn identifies_only_subscription_backed_o_auth_flows_as_subscriptions() {
		for oauth in [
			anthropic_oauth(),
			openai_chat_gpt_oauth(),
			openai_codex_oauth(),
			github_copilot_oauth(),
			kimi_coding_oauth(),
			xai_oauth(),
		] {
			assert_eq!(oauth.is_subscription, Some(true), "{}", oauth.name);
		}
		assert_ne!(open_router_oauth().is_subscription, Some(true));
	}

	#[test]
	fn anthropic_to_auth_derives_the_api_key_from_the_access_token() {
		let auth = to_auth(anthropic_oauth(), credential("token", "r", 0.0));
		assert_eq!(auth_json(&auth), json!({ "apiKey": "token" }));
	}

	#[test]
	fn openai_codex_to_auth_derives_the_api_key_from_the_access_token() {
		let auth = to_auth(openai_codex_oauth(), credential("token", "r", 0.0));
		assert_eq!(auth_json(&auth), json!({ "apiKey": "token" }));
	}

	#[test]
	fn openrouter_derives_the_api_key_and_keeps_the_permanent_credential_on_refresh() {
		let stored = credential("token", "", 9_007_199_254_740_991.0);
		assert_eq!(
			auth_json(&to_auth(open_router_oauth(), stored.clone())),
			json!({ "apiKey": "token" })
		);
		let refreshed = common::run_with_fake_timers(Rc::new(|_| unreachable!("refresh does not fetch")), || {
			open_router_oauth().refresh(stored.clone(), AbortController::new().signal())
		})
		.unwrap();
		assert_eq!(refreshed, stored);
	}

	#[test]
	fn x_ai_to_auth_derives_the_api_key_from_the_access_token() {
		let auth = to_auth(xai_oauth(), credential("token", "r", 0.0));
		assert_eq!(auth_json(&auth), json!({ "apiKey": "token" }));
	}

	#[test]
	fn github_copilot_to_auth_derives_base_url_from_the_token_proxy_endpoint() {
		let access = "tid=abc;exp=123;proxy-ep=proxy.enterprise.example;rest";
		let auth = to_auth(github_copilot_oauth(), credential(access, "r", 0.0));
		assert_eq!(
			auth_json(&auth),
			json!({ "apiKey": access, "baseUrl": "https://api.enterprise.example" })
		);
	}

	#[test]
	fn github_copilot_to_auth_falls_back_to_the_enterprise_domain_then_the_individual_endpoint() {
		let mut enterprise_credential = credential("no-proxy-ep", "r", 0.0);
		enterprise_credential.set_field("enterpriseUrl", json!("https://company.ghe.com"));
		let enterprise = to_auth(github_copilot_oauth(), enterprise_credential);
		assert_eq!(
			enterprise.base_url.as_deref(),
			Some("https://copilot-api.company.ghe.com")
		);

		let individual = to_auth(github_copilot_oauth(), credential("no-proxy-ep", "r", 0.0));
		assert_eq!(
			individual.base_url.as_deref(),
			Some("https://api.individual.githubcopilot.com")
		);
	}

	#[test]
	fn anthropic_refresh_exchanges_the_refresh_token_and_returns_a_typed_credential() {
		let (refreshed, now) = common::run_with_fake_timers(
			Rc::new(|_| {
				Box::pin(async {
					Ok(json_response(
						json!({ "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600 }),
						200,
					))
				})
			}),
			|| async {
				let refreshed = anthropic_oauth()
					.refresh(credential("old", "old-r", 0.0), AbortController::new().signal())
					.await;
				(refreshed, pi_host::now_ms())
			},
		);
		let refreshed = refreshed.unwrap();
		assert_eq!(serde_json::to_value(&refreshed).unwrap()["type"], "oauth");
		assert_eq!(refreshed.access, "new-access");
		assert_eq!(refreshed.refresh, "new-refresh");
		assert!(refreshed.expires > now);
	}

	#[test]
	fn github_copilot_refresh_preserves_the_enterprise_domain() {
		let fetched_urls = Rc::new(RefCell::new(Vec::<String>::new()));
		let recorded = fetched_urls.clone();
		let mut stored = credential("old", "gh-token", 0.0);
		stored.set_field("enterpriseUrl", json!("company.ghe.com"));
		let refreshed = common::run_with_fake_timers(
			Rc::new(move |request| {
				recorded.borrow_mut().push(request.url.clone());
				let response = if request.url.ends_with("/models") {
					json_response(json!({ "data": [] }), 200)
				} else {
					json_response(json!({ "token": "new-token", "expires_at": 9_999_999_999u64 }), 200)
				};
				Box::pin(async { Ok(response) })
			}),
			|| github_copilot_oauth().refresh(stored, AbortController::new().signal()),
		)
		.unwrap();
		assert_eq!(refreshed.access, "new-token");
		assert_eq!(refreshed.field("enterpriseUrl"), Some(&json!("company.ghe.com")));
		assert!(fetched_urls.borrow()[0].contains("api.company.ghe.com"));
	}
}
