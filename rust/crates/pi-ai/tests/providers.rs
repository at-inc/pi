//! Port of `packages/ai/test/providers.test.ts`.
//!
//! `getModel`/`getModels` (compat.ts) are aliases of `get_builtin_model`/`get_builtin_models`, so
//! `returns empty results for unknown provider ids` checks the `all.rs` getters once. `fauxAssistantMessage`
//! (providers/faux.ts, pending) is built inline as a text-only assistant message.
//!
//! Not ported: the `fauxProvider` describe (`streams queued responses through a Models collection`, `submits, polls,
//! and redeems deferred responses`, `records cancellation and returns deferred fetch failures in-band`). It tests
//! `providers/faux.ts`, which is pending in w2-ai-core and moves with that file.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::lazy::{LazyApiCapabilities, lazy_api};
use pi_ai::auth::helpers::env_api_key_auth;
use pi_ai::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthEvent, AuthPrompt, AuthResult, ModelAuth,
	ProviderAuth, ProviderAuthInteraction,
};
use pi_ai::models::{
	CreateModelsOptions, CreateProviderOptions, GetAuthTarget, ModelsRefreshOptions, ModelsSimpleStreamOptions,
	MutableModels, ProviderApi, create_models, create_provider, get_supported_thinking_levels,
};
use pi_ai::models_store::{InMemoryModelsStore, ModelsStore};
use pi_ai::providers::all::{
	builtin_models, builtin_providers, get_all_builtin_models, get_builtin_classifier_model,
	get_builtin_classifier_models, get_builtin_image_model, get_builtin_image_models, get_builtin_model,
	get_builtin_models, get_builtin_providers,
};
use pi_ai::providers::amazon_bedrock::amazon_bedrock_provider;
use pi_ai::providers::anthropic::anthropic_provider;
use pi_ai::providers::cloudflare_ai_gateway::cloudflare_ai_gateway_provider;
use pi_ai::providers::cloudflare_workers_ai::cloudflare_workers_ai_provider;
use pi_ai::providers::google_vertex::google_vertex_provider;
use pi_ai::providers::model_schema::ModelThinkingLevel;
use pi_ai::types::FetchDeferredFunction;
use pi_ai::types::{
	AnyModel, AssistantContent, AssistantMessage, AssistantMessageEvent, BaseModel, Context, DeferredHandle, Message,
	Model, ModelCost, ModelInputModality, ProviderEnv, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions,
	StopReason, TextContent, TranscriptContext, UserMessage, UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::{AbortController, LocalBoxFuture, PiResult};
use serde_json::{Value, json};

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual), Value::Number(expected)) => assert_eq!(actual.as_f64(), expected.as_f64()),
		_ => assert_eq!(actual, expected),
	}
}

struct FakeAuthContext {
	env: IndexMap<String, String>,
	files: Vec<String>,
}

impl AuthContext for FakeAuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>> {
		let value = self.env.get(name).cloned();
		Box::pin(async move { value })
	}

	fn file_exists(&self, path: &str) -> LocalBoxFuture<'static, bool> {
		let exists = self.files.iter().any(|file| file == path);
		Box::pin(async move { exists })
	}
}

fn fake_auth_context(env: &[(&str, &str)], files: &[&str]) -> Rc<dyn AuthContext> {
	Rc::new(FakeAuthContext {
		env: env
			.iter()
			.map(|(name, value)| (name.to_string(), value.to_string()))
			.collect(),
		files: files.iter().map(|file| file.to_string()).collect(),
	})
}

fn provider_env(entries: &[(&str, &str)]) -> ProviderEnv {
	entries
		.iter()
		.map(|(name, value)| (name.to_string(), value.to_string()))
		.collect()
}

fn models_with_auth_context(env: &[(&str, &str)], files: &[&str]) -> Rc<dyn MutableModels> {
	create_models(Some(CreateModelsOptions {
		auth_context: Some(fake_auth_context(env, files)),
		..CreateModelsOptions::default()
	}))
}

async fn get_auth(models: &dyn MutableModels, provider: &str) -> Option<AuthResult> {
	models.get_auth(GetAuthTarget::Provider(provider), None).await.unwrap()
}

fn never_aborted_signal() -> pi_host::AbortSignal {
	AbortController::new().signal()
}

fn user_context() -> Context {
	Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("hi".into()),
			timestamp: pi_host::now_ms(),
			..UserMessage::default()
		})],
		..Context::default()
	}
}

fn context() -> TranscriptContext {
	normalize_context(&user_context())
}

/// An interaction whose prompts answer from `answers` in order and whose events land in `events`.
fn scripted_interaction(answers: &[&str], events: Rc<RefCell<Vec<AuthEvent>>>) -> ProviderAuthInteraction {
	let answers = Rc::new(RefCell::new(
		answers.iter().map(|answer| answer.to_string()).collect::<VecDeque<_>>(),
	));
	ProviderAuthInteraction {
		signal: never_aborted_signal(),
		prompt: Rc::new(move |_| {
			let answer = answers.borrow_mut().pop_front().expect("an answer for every prompt");
			Box::pin(async move { Ok(answer) })
		}),
		notify: Rc::new(move |event| events.borrow_mut().push(event)),
	}
}

fn api_key_auth(provider: pi_ai::models::Provider) -> ApiKeyAuth {
	provider.auth.api_key.expect("the provider has api-key auth")
}

fn default_image_resize() -> Value {
	json!({ "maxWidth": 2000, "maxHeight": 2000, "maxBytes": 4.5 * 1024.0 * 1024.0, "jpegQuality": 80 })
}

fn builtin(provider: &str, id: &str) -> Model {
	get_builtin_model(provider, id).unwrap_or_else(|| panic!("{provider}/{id} is missing"))
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

mod builtin_providers {
	use super::*;
	use ModelThinkingLevel::{High, Low, Medium, Minimal};

	#[test]
	fn builtin_models_registers_every_builtin_provider_with_models() {
		let models = builtin_models(None);
		let providers = models.get_providers();
		assert_eq!(providers.len(), builtin_providers().len());
		assert!(providers.iter().any(|provider| provider.id == "anthropic"));

		let anthropic = models.get_model("anthropic", "claude-haiku-4-5");
		assert_eq!(
			anthropic.map(|model| model.api.clone()),
			Some("anthropic-messages".into())
		);

		let all = models.get_models(None);
		assert!(all.len() > 500);

		for provider in &providers {
			let list = models.get_all_models(Some(&provider.id));
			assert!(!list.is_empty(), "{}", provider.id);
			assert!(
				list.iter()
					.all(|model| to_json(model)["provider"] == json!(provider.id))
			);
		}
		assert_match_object(
			&to_json(builtin("radius", "balanced")),
			&json!({ "api": "pi-messages", "provider": "radius" }),
		);
	}

	#[test]
	fn returns_empty_results_for_unknown_provider_ids() {
		assert_eq!(get_builtin_model("not-a-provider", "x"), None);
		assert_eq!(get_builtin_image_model("not-a-provider", "x"), None);
		assert_eq!(get_builtin_classifier_model("not-a-provider", "x"), None);
		assert_eq!(get_builtin_models("not-a-provider"), vec![]);
		assert_eq!(get_builtin_image_models("not-a-provider"), vec![]);
		assert_eq!(get_builtin_classifier_models("not-a-provider"), vec![]);
		assert_eq!(get_all_builtin_models("not-a-provider"), vec![]);
	}

	#[test]
	fn stores_native_constrained_sampling_capabilities_in_model_metadata() {
		let gpt4o = to_json(&builtin("openai", "gpt-4o").compat);
		assert_eq!(gpt4o["supportsStrictMode"], json!(true));
		assert_eq!(gpt4o.get("supportsOpenAIGrammarTools"), None);
		assert_match_object(
			&to_json(&builtin("openai", "gpt-5.4").compat),
			&json!({ "supportsStrictMode": true, "supportsOpenAIGrammarTools": true }),
		);
		assert_eq!(
			to_json(&builtin("anthropic", "claude-haiku-4-5").compat)["supportsStrictTools"],
			json!(true)
		);
	}

	#[test]
	fn keeps_the_conservative_resize_profile_on_every_vision_model() {
		let vision_models: Vec<Model> = get_builtin_providers()
			.into_iter()
			.flat_map(get_builtin_models)
			.filter(|model| model.input.contains(&ModelInputModality::Image))
			.collect();
		assert!(!vision_models.is_empty());
		for model in vision_models {
			let resize = &to_json(&model.input_limits)["images"]["resize"];
			assert_match_object(resize, &default_image_resize());
			assert_eq!(
				resize.as_object().map(|resize| resize.len()),
				Some(4),
				"{}/{}",
				model.provider,
				model.id
			);
		}
	}

	#[test]
	fn records_known_direct_provider_image_request_limits() {
		assert_match_object(
			&to_json(&builtin("anthropic", "claude-haiku-4-5").input_limits),
			&json!({ "maxRequestBytes": 32 * 1024 * 1024, "images": { "maxPerRequest": 100 } }),
		);
		assert_eq!(
			to_json(&builtin("anthropic", "claude-opus-5").input_limits)["images"]["maxPerRequest"].as_f64(),
			Some(600.0)
		);
		assert_match_object(
			&to_json(&builtin("amazon-bedrock", "anthropic.claude-haiku-4-5-20251001-v1:0").input_limits),
			&json!({ "images": { "maxPerMessage": 20 } }),
		);
		assert_match_object(
			&to_json(&builtin("openai", "gpt-4o").input_limits),
			&json!({ "maxRequestBytes": 512 * 1024 * 1024, "images": { "maxPerRequest": 1500 } }),
		);
		assert_match_object(
			&to_json(&builtin("google", "gemini-2.5-flash").input_limits),
			&json!({ "maxRequestBytes": 20 * 1024 * 1024, "images": { "maxPerRequest": 3600 } }),
		);
	}

	#[test]
	fn does_not_infer_image_limits_from_gateway_api_compatibility() {
		let open_router_model = get_builtin_models("openrouter")
			.into_iter()
			.find(|model| model.input.contains(&ModelInputModality::Image))
			.unwrap();
		assert_match_object(
			&to_json(&open_router_model.input_limits),
			&json!({ "images": { "resize": default_image_resize() } }),
		);
		assert_eq!(to_json(&open_router_model.input_limits).as_object().unwrap().len(), 1);
		assert_eq!(
			to_json(&open_router_model.input_limits)["images"]
				.as_object()
				.unwrap()
				.len(),
			1
		);
	}

	#[test]
	fn uses_models_dev_effort_levels_for_google_thinking_models() {
		// Regression test for https://github.com/earendil-works/pi/issues/9455
		for provider in ["google", "google-vertex"] {
			assert!(get_supported_thinking_levels(&builtin(provider, "gemini-3.6-flash")).contains(&Minimal));
			assert_eq!(
				get_supported_thinking_levels(&builtin(provider, "gemini-3.8-flash")),
				[Low, Medium, High]
			);
			assert_eq!(
				get_supported_thinking_levels(&builtin(provider, "gemini-3.1-pro-preview")),
				[Low, Medium, High]
			);
		}
		assert_eq!(
			get_supported_thinking_levels(&builtin("opencode", "gemini-3.8-flash")),
			[Low, Medium, High]
		);
		assert_eq!(
			get_supported_thinking_levels(&builtin("google", "gemma-4-31b-it")),
			[Minimal, High]
		);
	}

	#[test]
	fn enables_mid_conversation_system_messages_only_for_verified_models() {
		let models = builtin_models(None);
		let supported = [
			("moonshotai", "kimi-k2.6"),
			("moonshotai", "kimi-k2.7-code"),
			("moonshotai", "kimi-k2.7-code-highspeed"),
			("moonshotai", "kimi-k3"),
			("moonshotai-cn", "kimi-k2.6"),
			("moonshotai-cn", "kimi-k2.7-code"),
			("moonshotai-cn", "kimi-k2.7-code-highspeed"),
			("moonshotai-cn", "kimi-k3"),
			("fireworks", "accounts/fireworks/models/kimi-k3"),
			("fireworks", "accounts/fireworks/routers/kimi-k3-fast"),
			("openai", "gpt-5.4"),
			("openai", "gpt-5.5"),
			("openai", "gpt-6-astra"),
			("openai-codex", "gpt-5.6-sol"),
			("anthropic", "claude-opus-5"),
			("opencode", "gpt-5.4"),
			("opencode", "gpt-5.6-terra"),
			("opencode-go", "gpt-5.6-luna"),
			("opencode", "claude-opus-4-8"),
			("opencode", "claude-opus-5"),
			("opencode", "kimi-k3"),
			("opencode-go", "kimi-k3"),
			("github-copilot", "gpt-5.6-terra"),
			("github-copilot", "claude-opus-5"),
			("github-copilot", "claude-opus-4.8"),
			("github-copilot", "kimi-k3"),
			("deepseek", "deepseek-v4-pro"),
			("openrouter", "openai/gpt-5.6-terra"),
		];
		let unsupported = [
			("fireworks", "accounts/fireworks/models/nemotron-3-ultra-nvfp4"),
			("openai", "gpt-4.1"),
			("openai", "gpt-5.2"),
			("anthropic", "claude-sonnet-4-5"),
			("google", "gemini-2.5-pro"),
			("opencode", "gpt-5.2"),
			("opencode", "claude-sonnet-4-5"),
			("github-copilot", "claude-sonnet-4.6"),
			("deepseek", "deepseek-flash"),
			("openrouter", "anthropic/claude-opus-5"),
			("openrouter", "moonshotai/kimi-k3"),
			("openrouter", "openai/gpt-5.6-terra:batch"),
		];
		for (provider, model_id) in supported {
			let model = models
				.get_model(provider, model_id)
				.unwrap_or_else(|| panic!("{provider}/{model_id}"));
			assert_eq!(
				to_json(model.compat)["supportsMidConvoSystemMessages"],
				json!(true),
				"{provider}/{model_id}"
			);
		}
		for (provider, model_id) in unsupported {
			let model = models
				.get_model(provider, model_id)
				.unwrap_or_else(|| panic!("{provider}/{model_id}"));
			assert_eq!(
				to_json(model.compat).get("supportsMidConvoSystemMessages"),
				None,
				"{provider}/{model_id}"
			);
		}
	}

	#[test]
	fn routes_proxied_tool_changes_through_verified_transports_only() {
		let models = builtin_models(None);
		let compat = |provider: &str, model_id: &str| to_json(&models.get_model(provider, model_id).unwrap().compat);
		for (provider, model_id) in [("opencode", "gpt-5.6-terra"), ("github-copilot", "gpt-5.6-terra")] {
			// Proxies pass `additional_tools` through to OpenAI but are not verified for tool search.
			assert_eq!(
				compat(provider, model_id)["supportsAdditionalTools"],
				json!(true),
				"{provider}/{model_id}"
			);
			assert_eq!(
				compat(provider, model_id).get("supportsToolSearch"),
				None,
				"{provider}/{model_id}"
			);
		}
		// Proxied Anthropic endpoints reject `tool_addition`/`tool_removal` blocks.
		for provider in ["opencode", "github-copilot"] {
			assert_eq!(
				compat(provider, "claude-opus-5").get("supportsMidConvoToolChanges"),
				None,
				"{provider}"
			);
		}
		assert_eq!(
			compat("anthropic", "claude-opus-5")["supportsMidConvoToolChanges"],
			json!(true)
		);
		// Kimi-style tool-bearing system messages survive Moonshot and OpenCode but not Copilot.
		for provider in ["moonshotai", "moonshotai-cn", "opencode", "opencode-go"] {
			assert_eq!(
				compat(provider, "kimi-k3")["supportsMidConvoToolAdditions"],
				json!(true),
				"{provider}"
			);
		}
		for provider in ["moonshotai", "moonshotai-cn"] {
			for model_id in ["kimi-k2.6", "kimi-k2.7-code", "kimi-k2.7-code-highspeed"] {
				assert_eq!(
					compat(provider, model_id).get("supportsMidConvoToolAdditions"),
					None,
					"{provider}/{model_id}"
				);
			}
		}
		assert_eq!(
			compat("github-copilot", "kimi-k3").get("supportsMidConvoToolAdditions"),
			None
		);
		assert_eq!(
			compat("openrouter", "openai/gpt-5.6-terra").get("supportsMidConvoToolAdditions"),
			None
		);
	}

	#[test]
	fn uses_official_kimi_k3_pricing_for_moonshot_providers() {
		let models = builtin_models(None);
		for provider in ["moonshotai", "moonshotai-cn"] {
			assert_eq!(
				to_json(&models.get_model(provider, "kimi-k3").unwrap().cost),
				json!({ "input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.0 }),
				"{provider}"
			);
		}
	}

	#[test]
	fn uses_api_equivalent_implied_pricing_for_kimi_coding_subscription_models() {
		let models = builtin_models(None);
		let expected_costs = [
			(
				"k3",
				json!({ "input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.0 }),
			),
			(
				"kimi-for-coding-highspeed",
				json!({ "input": 1.9, "output": 8.0, "cacheRead": 0.38, "cacheWrite": 0.0 }),
			),
		];

		for (model_id, cost) in expected_costs {
			assert_eq!(
				to_json(&models.get_model("kimi-coding", model_id).unwrap().cost),
				cost,
				"{model_id}"
			);
		}
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn resolves_anthropic_bearer_auth_from_env_with_auth_token_precedence() {
		pi_host::testing::run(|_| async {
			let models = models_with_auth_context(
				&[
					("ANTHROPIC_AUTH_TOKEN", "auth-token"),
					("ANTHROPIC_OAUTH_TOKEN", "oauth-token"),
					("ANTHROPIC_API_KEY", "api-key"),
				],
				&[],
			);
			models.set_provider(Rc::new(anthropic_provider()));

			assert_eq!(
				get_auth(models.as_ref(), "anthropic").await,
				Some(AuthResult {
					auth: ModelAuth {
						headers: Some(IndexMap::from([(
							"Authorization".to_string(),
							Some("Bearer auth-token".to_string())
						)])),
						..ModelAuth::default()
					},
					env: None,
					source: Some("ANTHROPIC_AUTH_TOKEN".into()),
				})
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn preserves_anthropic_oauth_token_precedence_over_the_api_key() {
		pi_host::testing::run(|_| async {
			let models = models_with_auth_context(
				&[("ANTHROPIC_API_KEY", "key"), ("ANTHROPIC_OAUTH_TOKEN", "oauth-token")],
				&[],
			);
			models.set_provider(Rc::new(anthropic_provider()));

			let result = get_auth(models.as_ref(), "anthropic").await.unwrap();
			assert_eq!(result.auth.api_key.as_deref(), Some("oauth-token"));
			assert_eq!(result.source.as_deref(), Some("ANTHROPIC_OAUTH_TOKEN"));
		});
	}

	#[test]
	fn runs_provider_owned_bedrock_bearer_token_and_aws_profile_login_flows() {
		pi_host::testing::run(|_| async {
			let auth = api_key_auth(amazon_bedrock_provider());
			let login = auth.login.clone().unwrap();
			let ignored = Rc::new(RefCell::new(Vec::new()));
			assert_eq!(
				login(scripted_interaction(&["bearer-token", "bedrock-token"], ignored))
					.await
					.unwrap(),
				ApiKeyCredential {
					key: Some("bedrock-token".into()),
					env: None,
				}
			);

			let events = Rc::new(RefCell::new(Vec::new()));
			assert_eq!(
				login(scripted_interaction(&["aws-profile", "work"], events.clone()))
					.await
					.unwrap(),
				ApiKeyCredential {
					key: None,
					env: Some(provider_env(&[("AWS_PROFILE", "work")])),
				}
			);
			let events = to_json(&*events.borrow());
			assert_eq!(events.as_array().unwrap().len(), 1);
			assert_eq!(events[0]["type"], json!("info"));
			assert_eq!(events[0]["links"].as_array().unwrap().len(), 1);
			assert_eq!(events[0]["links"][0]["label"], json!("AWS credential provider chain"));
			let resolved = (auth.resolve)(ApiKeyAuthInput {
				ctx: fake_auth_context(&[], &[]),
				credential: Some(ApiKeyCredential {
					key: None,
					env: Some(provider_env(&[("AWS_PROFILE", "work")])),
				}),
				signal: never_aborted_signal(),
			})
			.await
			.unwrap()
			.unwrap();
			assert_eq!(resolved.auth, ModelAuth::default());
			assert_eq!(resolved.env, Some(provider_env(&[("AWS_PROFILE", "work")])));
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn reports_bedrock_as_configured_from_ambient_aws_credentials_without_an_api_key() {
		pi_host::testing::run(|_| async {
			let models = models_with_auth_context(&[("AWS_PROFILE", "dev")], &[]);
			models.set_provider(Rc::new(amazon_bedrock_provider()));
			let model = models.get_models(Some("amazon-bedrock"))[0].clone();

			let result = get_auth(models.as_ref(), &model.provider).await.unwrap();
			assert_eq!(result.auth, ModelAuth::default());
			assert_eq!(result.source.as_deref(), Some("AWS_PROFILE"));

			let unconfigured = models_with_auth_context(&[], &[]);
			unconfigured.set_provider(Rc::new(amazon_bedrock_provider()));
			assert_eq!(get_auth(unconfigured.as_ref(), &model.provider).await, None);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn requires_cloudflare_workers_ai_account_config_and_returns_scoped_env() {
		pi_host::testing::run(|_| async {
			let missing_account = models_with_auth_context(&[("CLOUDFLARE_API_KEY", "cf-key")], &[]);
			missing_account.set_provider(Rc::new(cloudflare_workers_ai_provider()));
			let model = missing_account.get_models(Some("cloudflare-workers-ai"))[0].clone();
			assert_eq!(get_auth(missing_account.as_ref(), &model.provider).await, None);

			let configured = models_with_auth_context(
				&[
					("CLOUDFLARE_API_KEY", "cf-key"),
					("CLOUDFLARE_ACCOUNT_ID", "account-id"),
				],
				&[],
			);
			configured.set_provider(Rc::new(cloudflare_workers_ai_provider()));
			let result = get_auth(configured.as_ref(), &model.provider).await.unwrap();
			assert_eq!(
				result.auth,
				ModelAuth {
					api_key: Some("cf-key".into()),
					..ModelAuth::default()
				}
			);
			assert_eq!(
				result.env,
				Some(provider_env(&[("CLOUDFLARE_ACCOUNT_ID", "account-id")]))
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn requires_cloudflare_ai_gateway_account_and_gateway_config_and_returns_scoped_env_headers() {
		pi_host::testing::run(|_| async {
			let missing_gateway = models_with_auth_context(
				&[
					("CLOUDFLARE_API_KEY", "cf-key"),
					("CLOUDFLARE_ACCOUNT_ID", "account-id"),
				],
				&[],
			);
			missing_gateway.set_provider(Rc::new(cloudflare_ai_gateway_provider()));
			let model = missing_gateway.get_models(Some("cloudflare-ai-gateway"))[0].clone();
			assert_eq!(get_auth(missing_gateway.as_ref(), &model.provider).await, None);

			let configured = models_with_auth_context(
				&[
					("CLOUDFLARE_API_KEY", "cf-key"),
					("CLOUDFLARE_ACCOUNT_ID", "account-id"),
					("CLOUDFLARE_GATEWAY_ID", "gateway-id"),
				],
				&[],
			);
			configured.set_provider(Rc::new(cloudflare_ai_gateway_provider()));
			let result = get_auth(configured.as_ref(), &model.provider).await.unwrap();
			assert_eq!(
				result.auth,
				ModelAuth {
					headers: Some(IndexMap::from([
						("cf-aig-authorization".to_string(), Some("Bearer cf-key".to_string())),
						("Authorization".to_string(), None),
						("x-api-key".to_string(), None),
					])),
					..ModelAuth::default()
				}
			);
			assert_eq!(
				result.env,
				Some(provider_env(&[
					("CLOUDFLARE_ACCOUNT_ID", "account-id"),
					("CLOUDFLARE_GATEWAY_ID", "gateway-id"),
				]))
			);
		});
	}

	#[test]
	fn runs_provider_owned_vertex_api_key_and_adc_login_flows() {
		pi_host::testing::run(|_| async {
			let auth = api_key_auth(google_vertex_provider());
			let login = auth.login.clone().unwrap();
			let ignored = Rc::new(RefCell::new(Vec::new()));
			assert_eq!(
				login(scripted_interaction(&["api-key", "vertex-key"], ignored))
					.await
					.unwrap(),
				ApiKeyCredential {
					key: Some("vertex-key".into()),
					env: None,
				}
			);

			let events = Rc::new(RefCell::new(Vec::new()));
			let adc_env = provider_env(&[
				("GOOGLE_CLOUD_PROJECT", "project-id"),
				("GOOGLE_CLOUD_LOCATION", "us-central1"),
			]);
			assert_eq!(
				login(scripted_interaction(
					&["adc", "project-id", "us-central1"],
					events.clone()
				))
				.await
				.unwrap(),
				ApiKeyCredential {
					key: None,
					env: Some(adc_env.clone()),
				}
			);
			let events = to_json(&*events.borrow());
			assert_eq!(events.as_array().unwrap().len(), 1);
			assert_eq!(events[0]["type"], json!("info"));
			assert_eq!(events[0]["links"].as_array().unwrap().len(), 1);
			assert_eq!(events[0]["links"][0]["label"], json!("Application Default Credentials"));
			let resolved = (auth.resolve)(ApiKeyAuthInput {
				ctx: fake_auth_context(&[], &["~/.config/gcloud/application_default_credentials.json"]),
				credential: Some(ApiKeyCredential {
					key: None,
					env: Some(adc_env.clone()),
				}),
				signal: never_aborted_signal(),
			})
			.await
			.unwrap()
			.unwrap();
			assert_eq!(resolved.auth, ModelAuth::default());
			assert_eq!(resolved.env, Some(adc_env));
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn resolves_vertex_via_adc_file_plus_project_and_location() {
		pi_host::testing::run(|_| async {
			let adc = "~/.config/gcloud/application_default_credentials.json";
			let configured = models_with_auth_context(
				&[
					("GOOGLE_CLOUD_PROJECT", "proj"),
					("GOOGLE_CLOUD_LOCATION", "us-central1"),
				],
				&[adc],
			);
			configured.set_provider(Rc::new(google_vertex_provider()));
			let model = configured.get_models(Some("google-vertex"))[0].clone();

			let result = get_auth(configured.as_ref(), &model.provider).await.unwrap();
			assert_eq!(result.auth, ModelAuth::default());
			assert!(result.source.unwrap().contains("application default"));

			// ADC without project/location is not configured
			let partial = models_with_auth_context(&[("GOOGLE_CLOUD_PROJECT", "proj")], &[adc]);
			partial.set_provider(Rc::new(google_vertex_provider()));
			assert_eq!(get_auth(partial.as_ref(), &model.provider).await, None);

			// explicit key wins over ADC
			let keyed = models_with_auth_context(&[("GOOGLE_CLOUD_API_KEY", "vertex-key")], &[]);
			keyed.set_provider(Rc::new(google_vertex_provider()));
			assert_eq!(
				get_auth(keyed.as_ref(), &model.provider)
					.await
					.unwrap()
					.auth
					.api_key
					.as_deref(),
				Some("vertex-key")
			);
		});
	}
}

mod env_api_key_auth {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: envApiKeyAuth (auth/helpers.ts, w2-ai-auth)"]
	fn prefers_the_stored_credential_key_and_falls_back_through_env_vars_in_order() {
		pi_host::testing::run(|_| async {
			let auth = env_api_key_auth("Test key", &["FIRST_KEY", "SECOND_KEY"]);

			let stored = (auth.resolve)(ApiKeyAuthInput {
				ctx: fake_auth_context(&[("FIRST_KEY", "env")], &[]),
				credential: Some(ApiKeyCredential {
					key: Some("stored".into()),
					env: None,
				}),
				signal: never_aborted_signal(),
			})
			.await
			.unwrap()
			.unwrap();
			assert_eq!(stored.auth.api_key.as_deref(), Some("stored"));
			assert_eq!(stored.source.as_deref(), Some("stored credential"));

			let second = (auth.resolve)(ApiKeyAuthInput {
				ctx: fake_auth_context(&[("SECOND_KEY", "second")], &[]),
				credential: None,
				signal: never_aborted_signal(),
			})
			.await
			.unwrap()
			.unwrap();
			assert_eq!(second.auth.api_key.as_deref(), Some("second"));
			assert_eq!(second.source.as_deref(), Some("SECOND_KEY"));

			assert_eq!(
				(auth.resolve)(ApiKeyAuthInput {
					ctx: fake_auth_context(&[], &[]),
					credential: None,
					signal: never_aborted_signal(),
				})
				.await
				.unwrap(),
				None
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: envApiKeyAuth (auth/helpers.ts, w2-ai-auth)"]
	fn login_prompts_for_a_secret_and_returns_an_api_key_credential() {
		pi_host::testing::run(|_| async {
			let auth = env_api_key_auth("Test key", &["TEST_KEY"]);
			let credential = (auth.login.unwrap())(ProviderAuthInteraction {
				signal: never_aborted_signal(),
				prompt: Rc::new(|prompt| {
					assert!(matches!(prompt, AuthPrompt::Secret { .. }));
					Box::pin(async { Ok("entered-key".to_string()) })
				}),
				notify: Rc::new(|_| {}),
			})
			.await
			.unwrap();
			assert_eq!(
				credential,
				ApiKeyCredential {
					key: Some("entered-key".into()),
					env: None,
				}
			);
		});
	}
}

mod create_provider {
	use super::*;

	fn fake_assistant_message(text: &str) -> AssistantMessage {
		AssistantMessage {
			content: vec![AssistantContent::Text(TextContent {
				text: text.into(),
				..TextContent::default()
			})],
			api: "faux".into(),
			provider: "faux".into(),
			model: "faux-1".into(),
			stop_reason: StopReason::Stop,
			..AssistantMessage::default()
		}
	}

	fn completed_stream() -> AssistantMessageEventStream {
		let stream = AssistantMessageEventStream::new();
		let message = Rc::new(RefCell::new(fake_assistant_message("ok")));
		stream.push(AssistantMessageEvent::Start {
			partial: message.clone(),
		});
		stream.push(AssistantMessageEvent::Done {
			reason: StopReason::Stop,
			message: message.clone(),
		});
		stream.end(Some(message.borrow().clone()));
		stream
	}

	type Calls = Rc<RefCell<Vec<String>>>;
	type StreamOptionsSpy = Rc<dyn Fn(Option<&pi_ai::types::StreamOptions>)>;

	/// `recordingStreams(label, calls)`, with the optional deferred capabilities the TS test assigns afterwards.
	#[derive(Default)]
	struct RecordingStreams {
		label: String,
		calls: Calls,
		fetch_deferred: Option<FetchDeferredFunction>,
		cancel_deferred: Option<pi_ai::types::CancelDeferredFunction>,
		on_stream: Option<StreamOptionsSpy>,
	}

	impl RecordingStreams {
		fn respond(&self, model: &Model) -> PiResult<AssistantMessageEventStream> {
			self.calls.borrow_mut().push(format!("{}:{}", self.label, model.id));
			Ok(completed_stream())
		}
	}

	impl ProviderStreams for RecordingStreams {
		fn stream(
			&self,
			model: &Model,
			_context: &TranscriptContext,
			options: Option<ProviderStreamOptions>,
		) -> PiResult<AssistantMessageEventStream> {
			if let Some(on_stream) = &self.on_stream {
				on_stream(options.as_ref().map(|options| &options.base));
			}
			self.respond(model)
		}

		fn stream_simple(
			&self,
			model: &Model,
			_context: &TranscriptContext,
			options: Option<SimpleStreamOptions>,
		) -> PiResult<AssistantMessageEventStream> {
			if let Some(on_stream) = &self.on_stream {
				on_stream(options.as_ref().map(|options| &options.base));
			}
			self.respond(model)
		}

		fn fetch_deferred(&self) -> Option<FetchDeferredFunction> {
			self.fetch_deferred.clone()
		}

		fn cancel_deferred(&self) -> Option<pi_ai::types::CancelDeferredFunction> {
			self.cancel_deferred.clone()
		}
	}

	fn recording_streams(label: &str, calls: Calls) -> RecordingStreams {
		RecordingStreams {
			label: label.into(),
			calls,
			..RecordingStreams::default()
		}
	}

	fn test_model(api: &str, id: &str) -> Model {
		Model {
			base: BaseModel {
				id: id.into(),
				name: id.into(),
				api: api.into(),
				provider: "mixed".into(),
				base_url: "https://example.test/v1".into(),
				input: vec![ModelInputModality::Text],
				cost: ModelCost::default(),
				..BaseModel::default()
			},
			reasoning: false,
			context_window: 10000.0,
			max_tokens: 1000.0,
			..Model::default()
		}
	}

	fn no_auth() -> ProviderAuth {
		ProviderAuth {
			api_key: Some(ApiKeyAuth {
				name: "Test".into(),
				login: None,
				check: None,
				resolve: Rc::new(|_| Box::pin(async { Ok(Some(AuthResult::default())) })),
			}),
			oauth: None,
		}
	}

	#[test]
	fn lazily_exposes_only_declared_deferred_capabilities() {
		pi_host::testing::run(|_| async {
			let loads = Rc::new(RefCell::new(0));
			let counter = loads.clone();
			let api = lazy_api(
				Rc::new(move || {
					*counter.borrow_mut() += 1;
					Box::pin(async {
						let mut streams = recording_streams("deferred", Calls::default());
						streams.fetch_deferred = Some(Rc::new(|_, _, _| Ok(completed_stream())));
						Ok(Rc::new(streams) as Rc<dyn ProviderStreams>)
					})
				}),
				Some(LazyApiCapabilities {
					fetch_deferred: Some(true),
					cancel_deferred: None,
				}),
			);
			let model = test_model("api-a", "model-a");
			let handle = DeferredHandle {
				provider: model.provider.to_string(),
				model_id: model.id.clone(),
				api: model.api.to_string(),
				id: "response-1".into(),
				..DeferredHandle::default()
			};

			assert_eq!(*loads.borrow(), 0);
			assert!(api.cancel_deferred().is_none());
			let fetched = (api.fetch_deferred().unwrap())(&model, &handle, None).unwrap();
			assert_eq!(fetched.result().await.unwrap().stop_reason, StopReason::Stop);
			assert_eq!(*loads.borrow(), 1);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn dispatches_on_model_api_for_mixed_api_providers() {
		pi_host::testing::run(|_| async {
			let calls = Calls::default();
			let provider = create_provider(CreateProviderOptions {
				id: "mixed".into(),
				auth: no_auth(),
				models: vec![
					AnyModel::Chat(test_model("api-a", "model-a")),
					AnyModel::Chat(test_model("api-b", "model-b")),
				],
				api: Some(ProviderApi::ByApi(IndexMap::from([
					(
						"api-a".to_string(),
						Rc::new(recording_streams("a", calls.clone())) as Rc<dyn ProviderStreams>,
					),
					(
						"api-b".to_string(),
						Rc::new(recording_streams("b", calls.clone())) as Rc<dyn ProviderStreams>,
					),
				]))),
				..CreateProviderOptions::default()
			})
			.unwrap();
			let models = create_models(None);
			models.set_provider(Rc::new(provider));

			models
				.complete_simple(&test_model("api-a", "model-a"), &user_context(), None)
				.await;
			models
				.complete_simple(&test_model("api-b", "model-b"), &user_context(), None)
				.await;
			assert_eq!(*calls.borrow(), ["a:model-a", "b:model-b"]);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: resolveAuth (auth/resolve.ts, w2-ai-auth)"]
	fn merges_provider_resolved_env_into_stream_options() {
		pi_host::testing::run(|_| async {
			let captured: Rc<RefCell<(Option<ProviderEnv>, Option<String>)>> = Rc::default();
			let capture = captured.clone();
			let mut env_model = test_model("api-a", "model-a");
			env_model.base.provider = "env-provider".into();
			let provider = create_provider(CreateProviderOptions {
				id: "env-provider".into(),
				auth: ProviderAuth {
					api_key: Some(ApiKeyAuth {
						name: "Test".into(),
						login: None,
						check: None,
						resolve: Rc::new(|_| {
							Box::pin(async {
								Ok(Some(AuthResult {
									auth: ModelAuth {
										api_key: Some("provider-key".into()),
										..ModelAuth::default()
									},
									env: Some(provider_env(&[("PROVIDER_ONLY", "provider"), ("SHARED", "provider")])),
									source: None,
								}))
							})
						}),
					}),
					oauth: None,
				},
				models: vec![AnyModel::Chat(env_model.clone())],
				api: Some(ProviderApi::Single(Rc::new(RecordingStreams {
					label: "a".into(),
					on_stream: Some(Rc::new(move |options| {
						*capture.borrow_mut() = (
							options.and_then(|options| options.env.clone()),
							options.and_then(|options| options.api_key.clone()),
						);
					})),
					..RecordingStreams::default()
				}))),
				..CreateProviderOptions::default()
			})
			.unwrap();
			let models = create_models(None);
			models.set_provider(Rc::new(provider));

			let options = ModelsSimpleStreamOptions::from(SimpleStreamOptions {
				base: pi_ai::types::StreamOptions {
					base: pi_ai::types::ProviderRequestOptions {
						api_key: Some("request-key".into()),
						env: Some(provider_env(&[("REQUEST_ONLY", "request"), ("SHARED", "request")])),
						..Default::default()
					},
					..Default::default()
				},
				..SimpleStreamOptions::default()
			});
			models.complete_simple(&env_model, &user_context(), Some(options)).await;

			let (env, api_key) = captured.borrow().clone();
			assert_eq!(api_key.as_deref(), Some("request-key"));
			assert_eq!(
				env,
				Some(provider_env(&[
					("PROVIDER_ONLY", "provider"),
					("SHARED", "request"),
					("REQUEST_ONLY", "request"),
				]))
			);
		});
	}

	#[test]
	fn produces_a_stream_error_for_a_model_whose_api_has_no_implementation() {
		pi_host::testing::run(|_| async {
			let provider = create_provider(CreateProviderOptions {
				id: "mixed".into(),
				auth: no_auth(),
				models: vec![AnyModel::Chat(test_model("api-a", "model-a"))],
				api: Some(ProviderApi::ByApi(IndexMap::from([(
					"api-a".to_string(),
					Rc::new(recording_streams("a", Calls::default())) as Rc<dyn ProviderStreams>,
				)]))),
				..CreateProviderOptions::default()
			})
			.unwrap();
			let result = (provider.stream_simple)(&test_model("api-ghost", "model-x"), &context(), None)
				.unwrap()
				.result()
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert!(result.error_message.unwrap().contains("no API implementation"));
		});
	}

	#[test]
	#[ignore = "PORT-STUB: InMemoryCredentialStore, the default store of create_models (auth/credential-store.ts, w2-ai-auth)"]
	fn lets_a_newer_dynamic_refresh_bypass_and_supersede_older_network_work() {
		pi_host::testing::run(|_| async {
			let fetches = Rc::new(RefCell::new(0));
			let (mark_first_started, first_started) = futures::channel::oneshot::channel::<()>();
			let (finish_first, first_blocked) = futures::channel::oneshot::channel::<()>();
			let gates = Rc::new(RefCell::new(Some((mark_first_started, first_blocked))));
			let counter = fetches.clone();
			let provider = Rc::new(
				create_provider(CreateProviderOptions {
					id: "dynamic".into(),
					auth: no_auth(),
					models: vec![],
					fetch_models: Some(Rc::new(move |_| {
						*counter.borrow_mut() += 1;
						let current = *counter.borrow();
						let gate = if current == 1 { gates.borrow_mut().take() } else { None };
						Box::pin(async move {
							if let Some((mark_first_started, first_blocked)) = gate {
								let _ = mark_first_started.send(());
								let _ = first_blocked.await;
							}
							Ok(vec![AnyModel::Chat(test_model("api-a", &format!("listed-{current}")))])
						})
					})),
					api: Some(ProviderApi::Single(Rc::new(recording_streams("a", Calls::default())))),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			);

			let store = Rc::new(InMemoryModelsStore::new());
			let models = create_models(Some(CreateModelsOptions {
				models_store: Some(store.clone()),
				..CreateModelsOptions::default()
			}));
			models.set_provider(provider.clone());
			assert_eq!((provider.get_models)(), vec![]);

			let refresh = || ModelsRefreshOptions {
				providers: Some(vec!["dynamic".into()]),
				..ModelsRefreshOptions::default()
			};
			let first = models.refresh(Some(refresh()));
			let (first_done, first_result) = futures::channel::oneshot::channel();
			pi_host::spawn_local(async move {
				let _ = first_done.send(first.await);
			});
			first_started.await.unwrap();
			let second = models.refresh(Some(refresh())).await.unwrap();
			assert!(!second.aborted);
			let ids = |models: Vec<Model>| models.into_iter().map(|model| model.id.clone()).collect::<Vec<_>>();
			let stored_ids = |store: Rc<InMemoryModelsStore>| async move {
				store
					.read("dynamic", None)
					.await
					.unwrap()
					.unwrap()
					.models
					.into_iter()
					.map(|model| to_json(model)["id"].as_str().unwrap().to_string())
					.collect::<Vec<_>>()
			};
			assert_eq!(*fetches.borrow(), 2);
			assert_eq!(ids((provider.get_models)()), ["listed-2"]);
			assert_eq!(stored_ids(store.clone()).await, ["listed-2"]);

			assert!(!first_result.await.unwrap().unwrap().aborted);
			finish_first.send(()).unwrap();
			assert_eq!(ids((provider.get_models)()), ["listed-2"]);
			assert_eq!(stored_ids(store).await, ["listed-2"]);
		});
	}
}
