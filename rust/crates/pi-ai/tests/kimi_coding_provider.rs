//! Port of `packages/ai/test/kimi-coding-provider.test.ts`.
//!
//! Not ported: the `completeSimple` request at the end of `persists the %s region with an API key`. It
//! dispatches through the openai-completions API (wave-3, `w3-openai-completions`), so it moves with that API.
//! `vi.stubEnv(name, "")` is the empty `TestHost` environment. `it.each` cases run in a loop.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::auth::resolve::AuthResolutionOverrides;
use pi_ai::auth::types::{ApiKeyCredential, AuthInteraction, AuthPrompt, AuthResult, AuthType, Credential};
use pi_ai::models::{GetAuthTarget, MutableModels, create_models};
use pi_ai::providers::kimi_coding::kimi_coding_provider;
use pi_ai::types::ProviderEnv;

fn kimi_models() -> Rc<dyn MutableModels> {
	let models = create_models(None);
	models.set_provider(Rc::new(kimi_coding_provider()));
	models
}

async fn get_auth(models: &dyn MutableModels, overrides: Option<AuthResolutionOverrides>) -> Option<AuthResult> {
	models
		.get_auth(GetAuthTarget::Provider("kimi-coding"), overrides)
		.await
		.unwrap()
}

fn api_key_override(env: Option<ProviderEnv>) -> AuthResolutionOverrides {
	AuthResolutionOverrides {
		api_key: Some("test-key".into()),
		env,
		..AuthResolutionOverrides::default()
	}
}

mod kimi_code_api_key_regions {
	use super::*;

	// #10720
	#[test]
	#[ignore = "PORT-STUB: envApiKeyAuth login and InMemoryCredentialStore (auth/helpers.ts, auth/credential-store.ts, w2-ai-auth)"]
	fn persists_the_region_with_an_api_key() {
		for (region, domain) in [("global", "ai"), ("mainland-cn", "com")] {
			pi_host::testing::run(|host| async move {
				let models = kimi_models();
				let prompts = Rc::new(RefCell::new(Vec::new()));
				let recorded = prompts.clone();
				let credential = models
					.login(
						"kimi-coding",
						AuthType::ApiKey,
						AuthInteraction {
							signal: None,
							prompt: Rc::new(move |prompt| {
								let answer = match prompt {
									AuthPrompt::Select { .. } => {
										recorded.borrow_mut().push("select");
										region
									}
									AuthPrompt::Secret { .. } => {
										recorded.borrow_mut().push("secret");
										"test-key"
									}
									AuthPrompt::Text { .. } => {
										recorded.borrow_mut().push("text");
										"test-key"
									}
									AuthPrompt::ManualCode { .. } => {
										recorded.borrow_mut().push("manual_code");
										"test-key"
									}
								};
								Box::pin(async move { Ok(answer.to_string()) })
							}),
							notify: Rc::new(|_| {}),
						},
						None,
					)
					.await
					.unwrap();
				assert_eq!(*prompts.borrow(), ["select", "secret"]);
				assert_eq!(
					credential,
					Credential::ApiKey(ApiKeyCredential {
						key: Some("test-key".into()),
						env: Some(ProviderEnv::from([
							(
								"KIMI_CODE_OAUTH_HOST".to_string(),
								format!("https://auth.kimi.{domain}")
							),
							(
								"KIMI_CODE_BASE_URL".to_string(),
								format!("https://api.kimi.{domain}/coding/v1")
							),
						])),
					})
				);
				host.set_env("KIMI_CODE_OAUTH_HOST", "https://auth.example.com");
				host.set_env("KIMI_CODE_BASE_URL", "https://api.example.com");
				let auth = get_auth(models.as_ref(), None).await.unwrap().auth;
				assert_eq!(auth.api_key.as_deref(), Some("test-key"));
				assert_eq!(auth.base_url, Some(format!("https://api.kimi.{domain}/coding/v1")));
			});
		}
	}

	#[test]
	#[ignore = "PORT-STUB: InMemoryCredentialStore and resolveAuth (auth/credential-store.ts, auth/resolve.ts, w2-ai-auth)"]
	fn resolves_international_endpoints_from_scoped_configuration() {
		for (name, value) in [
			("KIMI_CODE_BASE_URL", "https://api.kimi.ai/coding"),
			("KIMI_CODE_BASE_URL", "https://api.kimi.ai/coding/v1/"),
			("KIMI_CODE_OAUTH_HOST", "https://auth.kimi.ai"),
			("KIMI_OAUTH_HOST", "https://auth.kimi.ai/"),
		] {
			pi_host::testing::run(|_| async move {
				let models = kimi_models();
				let env = ProviderEnv::from([(name.to_string(), value.to_string())]);
				let auth = get_auth(models.as_ref(), Some(api_key_override(Some(env))))
					.await
					.unwrap()
					.auth;
				assert_eq!(auth.api_key.as_deref(), Some("test-key"), "{name}={value}");
				assert_eq!(
					auth.base_url.as_deref(),
					Some("https://api.kimi.ai/coding/v1"),
					"{name}={value}"
				);
			});
		}
	}

	#[test]
	#[ignore = "PORT-STUB: InMemoryCredentialStore and resolveAuth (auth/credential-store.ts, auth/resolve.ts, w2-ai-auth)"]
	fn resolves_an_ambient_api_key_and_custom_endpoint() {
		pi_host::testing::run(|host| async move {
			host.set_env("KIMI_API_KEY", "env-key");
			host.set_env("KIMI_CODE_BASE_URL", "https://gateway.example.com/kimi/v1/");
			let models = kimi_models();
			let auth = get_auth(models.as_ref(), None).await.unwrap().auth;
			assert_eq!(auth.api_key.as_deref(), Some("env-key"));
			assert_eq!(auth.base_url.as_deref(), Some("https://gateway.example.com/kimi/v1"));
		});
	}

	#[test]
	#[ignore = "PORT-STUB: InMemoryCredentialStore and resolveAuth (auth/credential-store.ts, auth/resolve.ts, w2-ai-auth)"]
	fn preserves_the_model_endpoint_when_no_regional_override_is_configured() {
		pi_host::testing::run(|_| async move {
			let models = kimi_models();
			let auth = get_auth(models.as_ref(), Some(api_key_override(None)))
				.await
				.unwrap()
				.auth;
			assert_eq!(auth.api_key.as_deref(), Some("test-key"));
			assert_eq!(auth.base_url, None);
			assert!(get_auth(models.as_ref(), None).await.is_none());
		});
	}
}
