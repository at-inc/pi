//! Port of `packages/ai/test/identity.test.ts`.
//!
//! In `client identity in requests`, the providers' `api: { stream, streamSimple }` from `openai-completions.ts` is
//! `openai_completions_api()`, the lazy wrapper over the same two functions. `toBe(model)` on a model read is
//! equality; `Headers.get` is a case-insensitive lookup in the captured request headers.

use pi_ai::utils::identity::{
	ClientAttribution, ClientIdentity, ClientIdentityTarget, PI_ATTRIBUTION, UserAgentMode, UserAgentModeName,
	format_user_agent, get_attribution_headers, get_identity_headers, merge_client_identity,
};
use pi_ai::utils::version::PI_AI_VERSION;
use pi_host::PiResult;
use serde_json::{Value, json};

fn target<'a>(provider: &'a str, base_url: &'a str) -> ClientIdentityTarget<'a> {
	ClientIdentityTarget { provider, base_url }
}

fn acme(categories: Option<Vec<&str>>) -> ClientAttribution {
	ClientAttribution {
		name: "acme".into(),
		url: "https://acme.example".into(),
		categories: categories.map(|categories| Some(categories.into_iter().map(String::from).collect())),
	}
}

fn attributed(attribution: ClientAttribution) -> ClientIdentity {
	ClientIdentity {
		attribution: Some(Some(attribution)),
		..ClientIdentity::default()
	}
}

fn headers_json(headers: PiResult<indexmap::IndexMap<String, String>>) -> Value {
	serde_json::to_value(headers.unwrap()).unwrap()
}

#[test]
fn keeps_the_version_constant_in_sync_with_package_json() {
	let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../packages/ai/package.json");
	let pkg: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
	assert_eq!(pkg["version"], PI_AI_VERSION);
}

#[test]
fn formats_user_agents() {
	let platform = match std::env::consts::OS {
		"macos" => "darwin",
		"windows" => "win32",
		other => other,
	};
	let arch = match std::env::consts::ARCH {
		"x86_64" => "x64",
		"aarch64" => "arm64",
		other => other,
	};
	let runtime = format!("rust/{}", env!("CARGO_PKG_RUST_VERSION"));
	assert_eq!(
		format_user_agent(None),
		format!("pi/{PI_AI_VERSION} ({platform}; {runtime}; {arch})")
	);
	let versioned = ClientIdentity {
		version: Some(Some("9.9.9".into())),
		..ClientIdentity::default()
	};
	assert_eq!(
		format_user_agent(Some(&versioned)),
		format!("pi/9.9.9 ({platform}; {runtime}; {arch})")
	);
	let short = ClientIdentity {
		user_agent: Some(Some(UserAgentMode::Named(UserAgentModeName::Short))),
		version: Some(Some("9.9.9".into())),
		..ClientIdentity::default()
	};
	assert_eq!(format_user_agent(Some(&short)), "pi");
	let custom = ClientIdentity {
		user_agent: Some(Some(UserAgentMode::Custom {
			custom: "acme/1.0".into(),
		})),
		..ClientIdentity::default()
	};
	assert_eq!(format_user_agent(Some(&custom)), "acme/1.0");
}

#[test]
fn merges_identities_field_by_field() {
	assert_eq!(merge_client_identity(&[None, None]), None);
	let base = ClientIdentity {
		attribution: Some(Some(PI_ATTRIBUTION.clone())),
		user_agent: Some(Some(UserAgentMode::Named(UserAgentModeName::Short))),
		..ClientIdentity::default()
	};
	let versioned = ClientIdentity {
		version: Some(Some("1.0.0".into())),
		..ClientIdentity::default()
	};
	assert_eq!(
		merge_client_identity(&[Some(&base), Some(&versioned)]),
		Some(ClientIdentity {
			attribution: Some(Some(PI_ATTRIBUTION.clone())),
			user_agent: Some(Some(UserAgentMode::Named(UserAgentModeName::Short))),
			version: Some(Some("1.0.0".into())),
		})
	);
	let off = ClientIdentity {
		attribution: Some(None),
		..ClientIdentity::default()
	};
	let merged = merge_client_identity(&[Some(&attributed(PI_ATTRIBUTION.clone())), Some(&off)]).unwrap();
	assert_eq!(serde_json::to_value(merged).unwrap(), json!({ "attribution": null }));
}

#[test]
fn parses_identities_from_json() {
	let identity: ClientIdentity =
		serde_json::from_value(json!({ "userAgent": { "custom": "acme" }, "attribution": null })).unwrap();
	assert_eq!(
		identity,
		ClientIdentity {
			user_agent: Some(Some(UserAgentMode::Custom { custom: "acme".into() })),
			attribution: Some(None),
			version: None,
		}
	);
	let short: ClientIdentity = serde_json::from_value(json!({ "userAgent": "short" })).unwrap();
	assert_eq!(
		short.user_agent,
		Some(Some(UserAgentMode::Named(UserAgentModeName::Short)))
	);
	let unset: ClientIdentity = serde_json::from_value(json!({})).unwrap();
	assert_eq!(unset.attribution, None);
}

#[test]
fn sends_no_attribution_unless_enabled() {
	let open_router = target("openrouter", "https://openrouter.ai/api/v1");
	assert!(get_attribution_headers(open_router, None).unwrap().is_empty());
	let off = ClientIdentity {
		attribution: Some(None),
		..ClientIdentity::default()
	};
	assert!(get_attribution_headers(open_router, Some(&off)).unwrap().is_empty());
	assert_eq!(
		headers_json(get_identity_headers(open_router, None)),
		json!({ "User-Agent": format_user_agent(None) })
	);
}

#[test]
fn attributes_open_router_requests() {
	let open_router = target("openrouter", "https://openrouter.ai/api/v1");
	assert_eq!(
		headers_json(get_attribution_headers(
			open_router,
			Some(&attributed(PI_ATTRIBUTION.clone()))
		)),
		json!({
			"HTTP-Referer": "https://pi.dev",
			"X-OpenRouter-Title": "pi",
			"X-OpenRouter-Categories": "cli-agent",
		})
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			open_router,
			Some(&attributed(acme(Some(vec!["cli-agent", "cloud-agent"]))))
		)),
		json!({
			"HTTP-Referer": "https://acme.example",
			"X-OpenRouter-Title": "acme",
			"X-OpenRouter-Categories": "cli-agent,cloud-agent",
		})
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			open_router,
			Some(&attributed(acme(Some(vec![]))))
		)),
		json!({ "HTTP-Referer": "https://acme.example", "X-OpenRouter-Title": "acme" })
	);
	// Custom providers routed through OpenRouter, including legacy base URLs that are not valid URLs.
	let legacy = get_attribution_headers(
		target("custom", "not-a-url-openrouter.ai"),
		Some(&attributed(PI_ATTRIBUTION.clone())),
	)
	.unwrap();
	assert_eq!(legacy.get("X-OpenRouter-Title").map(String::as_str), Some("pi"));
}

#[test]
fn attributes_nvidia_nim_requests_with_a_capitalized_name_and_open_code_requests_with_the_name() {
	let pi = attributed(PI_ATTRIBUTION.clone());
	assert_eq!(
		headers_json(get_attribution_headers(
			target("nvidia", "https://example.test/v1"),
			Some(&pi)
		)),
		json!({ "X-BILLING-INVOKE-ORIGIN": "Pi" })
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			target("custom-nim", "https://integrate.api.nvidia.com/v1"),
			Some(&attributed(acme(None)))
		)),
		json!({ "X-BILLING-INVOKE-ORIGIN": "Acme" })
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			target("opencode", "https://opencode.ai/zen/v1"),
			Some(&pi)
		)),
		json!({ "x-opencode-client": "pi" })
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			target("custom", "https://opencode.ai/zen/v1"),
			Some(&attributed(acme(None)))
		)),
		json!({ "x-opencode-client": "acme" })
	);
}

#[test]
fn does_not_attribute_nvidia_models_routed_through_gateways_to_nvidia() {
	let pi = attributed(PI_ATTRIBUTION.clone());
	assert!(
		!get_attribution_headers(target("openrouter", "https://openrouter.ai/api/v1"), Some(&pi))
			.unwrap()
			.contains_key("X-BILLING-INVOKE-ORIGIN")
	);
	assert!(
		get_attribution_headers(
			target("vercel-ai-gateway", "https://ai-gateway.vercel.sh/v1"),
			Some(&pi)
		)
		.unwrap()
		.is_empty()
	);
}

#[test]
fn keeps_unknown_user_agent_modes_and_formats_them_as_detailed() {
	let identity: ClientIdentity = serde_json::from_value(json!({ "userAgent": "long" })).unwrap();
	assert_eq!(
		identity.user_agent,
		Some(Some(UserAgentMode::Named(UserAgentModeName::Other("long".into()))))
	);
	assert_eq!(format_user_agent(Some(&identity)), format_user_agent(None));
	assert_eq!(serde_json::to_value(&identity).unwrap(), json!({ "userAgent": "long" }));
}

#[test]
fn throws_like_the_ts_for_null_open_router_categories() {
	let identity: ClientIdentity = serde_json::from_value(
		json!({ "attribution": { "name": "acme", "url": "https://acme.example", "categories": null } }),
	)
	.unwrap();
	let error =
		get_attribution_headers(target("openrouter", "https://openrouter.ai/api/v1"), Some(&identity)).unwrap_err();
	assert_eq!(error.name, "TypeError");
	assert_eq!(error.message, "Cannot read properties of null (reading 'length')");
	assert!(get_identity_headers(target("openrouter", "https://openrouter.ai/api/v1"), Some(&identity)).is_err());
	assert_eq!(
		headers_json(get_attribution_headers(
			target("opencode", "https://opencode.ai/zen/v1"),
			Some(&identity)
		)),
		json!({ "x-opencode-client": "acme" })
	);
	assert_eq!(
		serde_json::to_value(&identity).unwrap()["attribution"]["categories"],
		Value::Null
	);
}

// Rust-only regression: `mergeClientIdentity` copies every defined field, so a `null` `userAgent` or `version` from
// JSON replaces the earlier value and formats as the default. Expected values come from running the TS.
#[test]
fn null_user_agent_and_version_replace_merged_values() {
	let base: ClientIdentity = serde_json::from_value(json!({ "userAgent": "short", "version": "9.9.9" })).unwrap();
	let cleared: ClientIdentity = serde_json::from_value(json!({ "userAgent": null, "version": null })).unwrap();
	let merged = merge_client_identity(&[Some(&base), Some(&cleared)]).unwrap();
	assert_eq!(
		serde_json::to_value(&merged).unwrap(),
		json!({ "userAgent": null, "version": null })
	);
	assert_eq!(format_user_agent(Some(&merged)), format_user_agent(None));
}

mod client_identity_in_requests {
	use std::cell::RefCell;
	use std::rc::Rc;

	use indexmap::IndexMap;
	use pi_ai::api::openai_completions::{OpenAICompletionsOptions, stream as stream_completions};
	use pi_ai::api::openai_completions_lazy::openai_completions_api;
	use pi_ai::auth::types::{ApiKeyAuth, AuthResult, ModelAuth, ProviderAuth};
	use pi_ai::models::{
		CreateModelsOptions, CreateProviderOptions, ModelsSimpleStreamOptions, MutableModels, ProviderApi,
		create_models, create_provider, with_client_identity_mutable,
	};
	use pi_ai::types::{
		Context, Model, ProviderHeaders, ProviderRequestOptions, SimpleStreamOptions, StreamOptions, TranscriptContext,
	};
	use pi_ai::utils::transcript::normalize_context;
	use pi_host::{FetchFunction, HttpResponse};

	use super::*;

	fn completions_model(provider: &str, base_url: &str, headers: Option<Value>) -> Model {
		let mut model = json!({
			"id": "test-model",
			"name": "Test Model",
			"api": "openai-completions",
			"provider": provider,
			"baseUrl": base_url,
			"reasoning": false,
			"input": ["text"],
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
			"contextWindow": 128000,
			"maxTokens": 4096,
		});
		if let Some(headers) = headers {
			model["headers"] = headers;
		}
		serde_json::from_value(model).unwrap()
	}

	/// `capturingFetch()`: `headers()` reads the headers of the last request, by lowercase name.
	struct Capture {
		fetch: FetchFunction,
		captured: Rc<RefCell<Option<IndexMap<String, String>>>>,
	}

	impl Capture {
		fn header(&self, name: &str) -> Option<String> {
			let captured = self.captured.borrow();
			let headers = captured.as_ref().expect("no request was sent");
			headers
				.iter()
				.find(|(key, _)| key.eq_ignore_ascii_case(name))
				.map(|(_, value)| value.clone())
		}
	}

	fn capturing_fetch() -> Capture {
		let captured: Rc<RefCell<Option<IndexMap<String, String>>>> = Rc::default();
		let recorder = captured.clone();
		let fetch: FetchFunction = Rc::new(move |request| {
			*recorder.borrow_mut() = Some(request.headers.clone());
			let chunks = [
				json!({ "id": "chatcmpl-identity", "choices": [{ "delta": { "content": "ok" }, "finish_reason": null, "index": 0 }] }),
				json!({
					"id": "chatcmpl-identity",
					"choices": [{ "delta": {}, "finish_reason": "stop", "index": 0 }],
					"usage": { "prompt_tokens": 1, "completion_tokens": 1 },
				}),
			];
			let body = format!(
				"{}\n\ndata: [DONE]\n\n",
				chunks
					.iter()
					.map(|chunk| format!("data: {}", pi_host::js::json_stringify(chunk)))
					.collect::<Vec<_>>()
					.join("\n\n")
			);
			Box::pin(async move {
				Ok(HttpResponse::from_bytes(
					200,
					IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
					body,
				))
			})
		});
		Capture { fetch, captured }
	}

	fn context() -> TranscriptContext {
		normalize_context(&request_context())
	}

	fn request_context() -> Context {
		serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] })).unwrap()
	}

	fn identity(value: Value) -> ClientIdentity {
		serde_json::from_value(value).unwrap()
	}

	fn completions_provider(id: &str, model: &Model) -> Rc<pi_ai::models::Provider> {
		Rc::new(
			create_provider(CreateProviderOptions {
				id: id.into(),
				auth: ProviderAuth {
					api_key: Some(ApiKeyAuth {
						name: "Test".into(),
						login: None,
						check: None,
						resolve: Rc::new(|_| {
							Box::pin(async {
								Ok(Some(AuthResult {
									auth: ModelAuth {
										api_key: Some("key".into()),
										..ModelAuth::default()
									},
									env: None,
									source: None,
								}))
							})
						}),
					}),
					oauth: None,
				},
				models: vec![pi_ai::types::AnyModel::Chat(model.clone())],
				api: Some(ProviderApi::Single(openai_completions_api())),
				..CreateProviderOptions::default()
			})
			.unwrap(),
		)
	}

	#[test]
	fn applies_the_identity_below_model_and_request_headers() {
		pi_host::testing::run(|_| async {
			let capture = capturing_fetch();
			let model = completions_model(
				"openrouter",
				"https://openrouter.ai/api/v1",
				Some(json!({ "X-OpenRouter-Title": "model-title" })),
			);
			let options = OpenAICompletionsOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some("key".into()),
						fetch: Some(capture.fetch.clone()),
						identity: Some(ClientIdentity {
							attribution: Some(Some(PI_ATTRIBUTION.clone())),
							user_agent: Some(Some(UserAgentMode::Named(UserAgentModeName::Short))),
							..ClientIdentity::default()
						}),
						headers: Some(ProviderHeaders::from([("HTTP-Referer".to_string(), None)])),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				},
				..OpenAICompletionsOptions::default()
			};
			stream_completions(&model, &context(), Some(options)).result().await;

			assert_eq!(capture.header("user-agent").as_deref(), Some("pi"));
			assert_eq!(capture.header("x-openrouter-title").as_deref(), Some("model-title"));
			assert_eq!(capture.header("http-referer"), None);
		});
	}

	#[test]
	fn keeps_impersonating_model_headers_over_the_identity_user_agent() {
		pi_host::testing::run(|_| async {
			let capture = capturing_fetch();
			let model = completions_model(
				"github-copilot",
				"https://api.githubcopilot.com",
				Some(json!({ "User-Agent": "GitHubCopilotChat/0.35.0" })),
			);
			let options = OpenAICompletionsOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some("key".into()),
						fetch: Some(capture.fetch.clone()),
						identity: Some(identity(json!({ "userAgent": { "custom": "acme" } }))),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				},
				..OpenAICompletionsOptions::default()
			};
			stream_completions(&model, &context(), Some(options)).result().await;

			assert_eq!(
				capture.header("user-agent").as_deref(),
				Some("GitHubCopilotChat/0.35.0")
			);
		});
	}

	#[test]
	fn merges_request_identity_over_the_models_and_wrapper_identities() {
		pi_host::testing::run(|_| async {
			let model = completions_model("openrouter", "https://openrouter.ai/api/v1", None);
			let attribution = |name: &str| json!({ "name": name, "url": "https://acme.example" });
			let models = create_models(Some(CreateModelsOptions {
				identity: Some(identity(
					json!({ "attribution": attribution("models"), "userAgent": "short" }),
				)),
				..CreateModelsOptions::default()
			}));
			models.set_provider(completions_provider("openrouter", &model));
			let send = |target: Rc<dyn MutableModels>, identity: Option<ClientIdentity>| {
				let model = model.clone();
				async move {
					let capture = capturing_fetch();
					let options = SimpleStreamOptions {
						base: StreamOptions {
							base: ProviderRequestOptions {
								fetch: Some(capture.fetch.clone()),
								identity,
								..ProviderRequestOptions::default()
							},
							..StreamOptions::default()
						},
						..SimpleStreamOptions::default()
					};
					target
						.stream_simple(
							&model,
							&request_context(),
							Some(ModelsSimpleStreamOptions::from(options)),
						)
						.result()
						.await;
					capture
				}
			};

			let from_models = send(models.clone(), None).await;
			assert_eq!(from_models.header("user-agent").as_deref(), Some("pi"));
			assert_eq!(from_models.header("x-openrouter-title").as_deref(), Some("models"));

			let wrapped = with_client_identity_mutable(
				models.clone(),
				identity(json!({ "attribution": attribution("wrapper") })),
			);
			let from_wrapper = send(wrapped.clone(), None).await;
			assert_eq!(from_wrapper.header("user-agent").as_deref(), Some("pi"));
			assert_eq!(from_wrapper.header("x-openrouter-title").as_deref(), Some("wrapper"));
			assert_eq!(wrapped.get_model("openrouter", "test-model"), Some(model.clone()));
			// A MutableModels stays mutable.
			wrapped.delete_provider("openrouter");
			assert!(models.get_provider("openrouter").is_none());
			models.set_provider(completions_provider("openrouter", &model));

			let from_request = send(wrapped, Some(identity(json!({ "attribution": null })))).await;
			assert_eq!(from_request.header("x-openrouter-title"), None);
			assert_eq!(from_request.header("user-agent").as_deref(), Some("pi"));
		});
	}

	#[test]
	fn sends_the_open_code_session_header_to_custom_providers_pointing_at_open_code() {
		pi_host::testing::run(|_| async {
			let model = completions_model("custom-opencode", "https://opencode.ai/zen/v1", None);
			let models = create_models(None);
			models.set_provider(completions_provider("custom-opencode", &model));
			let send = |headers: Option<ProviderHeaders>| {
				let (models, model) = (models.clone(), model.clone());
				async move {
					let capture = capturing_fetch();
					let options = SimpleStreamOptions {
						base: StreamOptions {
							base: ProviderRequestOptions {
								fetch: Some(capture.fetch.clone()),
								headers,
								..ProviderRequestOptions::default()
							},
							session_id: Some("session-1".into()),
							..StreamOptions::default()
						},
						..SimpleStreamOptions::default()
					};
					models
						.stream_simple(
							&model,
							&request_context(),
							Some(ModelsSimpleStreamOptions::from(options)),
						)
						.result()
						.await;
					capture
				}
			};

			assert_eq!(
				send(None).await.header("x-opencode-session").as_deref(),
				Some("session-1")
			);
			let configured =
				ProviderHeaders::from([("X-OpenCode-Session".to_string(), Some("configured".to_string()))]);
			assert_eq!(
				send(Some(configured)).await.header("x-opencode-session").as_deref(),
				Some("configured")
			);
		});
	}
}
