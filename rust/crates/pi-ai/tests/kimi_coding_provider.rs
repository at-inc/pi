//! Port of `packages/ai/test/kimi-coding-provider.test.ts`.
//!
//! The `fetch` option of the `completeSimple` request records each request's URL and `authorization` header; the
//! recorded list stands in for the in-handler `expect`s and `toHaveBeenCalledOnce`. `vi.stubEnv(name, "")` is the empty `TestHost` environment. `it.each` cases run in a loop.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::auth::resolve::AuthResolutionOverrides;
use pi_ai::auth::types::{ApiKeyCredential, AuthInteraction, AuthPromptKind, AuthResult, AuthType, Credential};
use pi_ai::models::{GetAuthTarget, ModelsSimpleStreamOptions, MutableModels, create_models};
use pi_ai::providers::kimi_coding::kimi_coding_provider;
use pi_ai::types::{Context, ProviderEnv, ProviderRequestOptions, SimpleStreamOptions, StopReason, StreamOptions};
use pi_host::{FetchFunction, HttpResponse};
use serde_json::json;

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
								let answer = match prompt.kind {
									AuthPromptKind::Select { .. } => {
										recorded.borrow_mut().push("select");
										region
									}
									AuthPromptKind::Secret { .. } => {
										recorded.borrow_mut().push("secret");
										"test-key"
									}
									AuthPromptKind::Text { .. } => {
										recorded.borrow_mut().push("text");
										"test-key"
									}
									AuthPromptKind::ManualCode { .. } => {
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
						extra: Default::default(),
					})
				);
				host.set_env("KIMI_CODE_OAUTH_HOST", "https://auth.example.com");
				host.set_env("KIMI_CODE_BASE_URL", "https://api.example.com");
				let auth = get_auth(models.as_ref(), None).await.unwrap().auth;
				assert_eq!(auth.api_key.as_deref(), Some("test-key"));
				assert_eq!(auth.base_url, Some(format!("https://api.kimi.{domain}/coding/v1")));
				let requests: Rc<RefCell<Vec<(String, Option<String>)>>> = Rc::default();
				let recorder = requests.clone();
				let request: FetchFunction = Rc::new(move |request| {
					recorder
						.borrow_mut()
						.push((request.url.clone(), request.header("authorization").map(str::to_string)));
					Box::pin(async {
						Ok(HttpResponse::from_bytes(
							200,
							IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
							"data: {\"id\":\"chat_test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
						))
					})
				});
				let context: Context = serde_json::from_value(json!({
					"messages": [{ "role": "user", "content": "Hello", "timestamp": 0 }],
				}))
				.unwrap();
				let result = models
					.complete_simple(
						&models.get_model("kimi-coding", "kimi-for-coding").unwrap(),
						&context,
						Some(ModelsSimpleStreamOptions::from(SimpleStreamOptions {
							base: StreamOptions {
								base: ProviderRequestOptions {
									fetch: Some(request),
									..ProviderRequestOptions::default()
								},
								..StreamOptions::default()
							},
							..SimpleStreamOptions::default()
						})),
					)
					.await
					.unwrap();
				assert_eq!(result.stop_reason, StopReason::Stop);
				assert_eq!(result.error_message, None);
				assert_eq!(
					*requests.borrow(),
					[(
						format!("https://api.kimi.{domain}/coding/v1/chat/completions"),
						Some("Bearer test-key".to_string())
					)]
				);
			});
		}
	}

	#[test]
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
