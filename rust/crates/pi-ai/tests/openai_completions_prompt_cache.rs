//! Port of `packages/ai/test/openai-completions-prompt-cache.test.ts`.
//!
//! The vitest file records the `create` params and the client's `defaultHeaders`; the port reads both from the
//! request the client sends, so `headers` are the request headers.

mod openai_completions_support;

use indexmap::IndexMap;
use openai_completions_support::{builtin_model, options, serve_chunks};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::{CacheRetention, Context, Message, Model, UserMessage, UserMessageContent};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct CaptureOptions {
	cache_retention: Option<CacheRetention>,
	session_id: Option<&'static str>,
	headers: Option<IndexMap<String, Option<String>>>,
}

fn create_model(overrides: impl FnOnce(&mut Model)) -> Model {
	let mut model = builtin_model("openai", "gpt-4o-mini");
	overrides(&mut model);
	model
}

fn compat(value: Value) -> Option<Map<String, Value>> {
	value.as_object().cloned()
}

fn stop_chunk() -> Value {
	json!({
		"id": "chatcmpl-test",
		"choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
		"usage": { "prompt_tokens": 1, "completion_tokens": 1 },
	})
}

/// Returns the request payload and the request whose headers the client sent.
async fn capture_request(
	host: &TestHost,
	capture: CaptureOptions,
	model: Model,
) -> (Map<String, Value>, RecordedRequest) {
	serve_chunks(host, vec![stop_chunk()]);
	let mut request = options("test-key");
	request.cache_retention = capture.cache_retention;
	request.session_id = capture.session_id.map(str::to_string);
	request.headers = capture.headers;
	stream(
		&model,
		&normalize_context(&Context {
			system_prompt: Some("sys".into()),
			messages: vec![Message::User(UserMessage {
				content: UserMessageContent::Text("hi".into()),
				timestamp: pi_host::now_ms(),
				..UserMessage::default()
			})],
			..Context::default()
		}),
		Some(request),
	)
	.result()
	.await
	.unwrap();
	let recorded = host.requests().last().cloned().expect("a request was sent");
	let payload = recorded.json().as_object().cloned().expect("an object payload");
	(payload, recorded)
}

fn session(id: &'static str) -> CaptureOptions {
	CaptureOptions {
		session_id: Some(id),
		..CaptureOptions::default()
	}
}

mod openai_completions_prompt_caching {
	use super::*;

	#[test]
	fn sets_prompt_cache_key_for_direct_openai_requests_when_caching_is_enabled() {
		run(|host| async move {
			let (payload, _) = capture_request(&host, session("session-123"), create_model(|_| {})).await;
			assert_eq!(payload.get("prompt_cache_key"), Some(&json!("session-123")));
			assert_eq!(payload.get("prompt_cache_retention"), None);
		});
	}

	#[test]
	fn sets_prompt_cache_retention_to_24h_for_direct_openai_requests_when_cache_retention_is_long() {
		run(|host| async move {
			let capture = CaptureOptions {
				cache_retention: Some(CacheRetention::Long),
				..session("session-456")
			};
			let (payload, _) = capture_request(&host, capture, create_model(|_| {})).await;
			assert_eq!(payload.get("prompt_cache_key"), Some(&json!("session-456")));
			assert_eq!(payload.get("prompt_cache_retention"), Some(&json!("24h")));
		});
	}

	#[test]
	fn clamps_prompt_cache_key_to_openais_64_character_limit() {
		run(|host| async move {
			let session_id: &'static str = Box::leak("x".repeat(67).into_boxed_str());
			let (payload, _) = capture_request(&host, session(session_id), create_model(|_| {})).await;
			assert_eq!(payload.get("prompt_cache_key"), Some(&json!("x".repeat(64))));
		});
	}

	#[test]
	fn omits_prompt_cache_fields_when_cache_retention_is_none() {
		run(|host| async move {
			let capture = CaptureOptions {
				cache_retention: Some(CacheRetention::None),
				..session("session-789")
			};
			let (payload, _) = capture_request(&host, capture, create_model(|_| {})).await;
			assert_eq!(payload.get("prompt_cache_key"), None);
			assert_eq!(payload.get("prompt_cache_retention"), None);
		});
	}

	#[test]
	fn omits_prompt_cache_fields_for_non_openai_base_urls_without_compatible_long_retention() {
		run(|host| async move {
			let model = create_model(|model| {
				model.base.base_url = "https://proxy.example.com/v1".into();
				model.compat = compat(json!({ "supportsLongCacheRetention": false }));
			});
			let capture = CaptureOptions {
				cache_retention: Some(CacheRetention::Long),
				..session("session-proxy")
			};
			let (payload, _) = capture_request(&host, capture, model).await;
			assert_eq!(payload.get("prompt_cache_key"), None);
			assert_eq!(payload.get("prompt_cache_retention"), None);
		});
	}

	#[test]
	fn uses_pi_cache_retention_for_direct_openai_requests() {
		run(|host| async move {
			host.set_env("PI_CACHE_RETENTION", "long");
			let (payload, _) = capture_request(&host, session("session-env"), create_model(|_| {})).await;
			assert_eq!(payload.get("prompt_cache_key"), Some(&json!("session-env")));
			assert_eq!(payload.get("prompt_cache_retention"), Some(&json!("24h")));
		});
	}

	#[test]
	fn sends_known_session_affinity_headers_when_compat_send_session_affinity_headers_is_enabled() {
		run(|host| async move {
			let model = create_model(|model| {
				model.base.base_url = "https://proxy.example.com/v1".into();
				model.compat = compat(json!({ "sendSessionAffinityHeaders": true }));
			});
			let (_, headers) = capture_request(&host, session("session-affinity"), model).await;
			assert_eq!(headers.header("session_id"), Some("session-affinity"));
			assert_eq!(headers.header("x-client-request-id"), Some("session-affinity"));
			assert_eq!(headers.header("x-session-affinity"), Some("session-affinity"));
		});
	}

	#[test]
	fn sends_fireworks_session_affinity_for_accounts_fireworks_models_glm_5p3() {
		sends_fireworks_session_affinity("accounts/fireworks/models/glm-5p3");
	}

	#[test]
	fn sends_fireworks_session_affinity_for_accounts_fireworks_routers_glm_5p3_fast() {
		sends_fireworks_session_affinity("accounts/fireworks/routers/glm-5p3-fast");
	}

	fn sends_fireworks_session_affinity(model_id: &'static str) {
		run(move |host| async move {
			let model = pi_ai::providers::all::get_builtin_model("fireworks", model_id).expect("built-in model");
			let (_, headers) = capture_request(&host, session("fireworks-session"), model).await;
			assert_eq!(headers.header("x-session-affinity"), Some("fireworks-session"));
		});
	}

	#[test]
	fn sends_baseten_session_affinity_for_built_in_catalog_models() {
		run(|host| async move {
			let model = pi_ai::providers::all::get_builtin_model("baseten", "zai-org/GLM-5.2").expect("built-in model");
			let (_, headers) = capture_request(&host, session("baseten-catalog-session"), model).await;
			assert_eq!(headers.header("x-session-affinity"), Some("baseten-catalog-session"));
			assert_eq!(headers.header("x-client-request-id"), Some("baseten-catalog-session"));
		});
	}

	#[test]
	fn uses_openai_no_session_format_when_configured() {
		run(|host| async move {
			let model = create_model(|model| {
				model.compat =
					compat(json!({ "sendSessionAffinityHeaders": true, "sessionAffinityFormat": "openai-nosession" }));
			});
			let (payload, headers) = capture_request(&host, session("session-nosession"), model).await;
			assert_eq!(payload.get("session_id"), None);
			assert_eq!(payload.get("prompt_cache_key"), Some(&json!("session-nosession")));
			assert_eq!(headers.header("session_id"), None);
			assert_eq!(headers.header("x-client-request-id"), Some("session-nosession"));
			assert_eq!(headers.header("x-session-affinity"), Some("session-nosession"));
			assert_eq!(headers.header("x-session-id"), None);
		});
	}

	#[test]
	fn uses_openrouter_session_affinity_header_when_configured() {
		run(|host| async move {
			let model = create_model(|model| {
				model.base.base_url = "https://proxy.example.com/v1".into();
				model.compat =
					compat(json!({ "sendSessionAffinityHeaders": true, "sessionAffinityFormat": "openrouter" }));
			});
			let (payload, headers) = capture_request(&host, session("session-proxy"), model).await;
			assert_eq!(payload.get("session_id"), None);
			assert_eq!(payload.get("prompt_cache_key"), None);
			assert_eq!(headers.header("x-session-id"), Some("session-proxy"));
			assert_eq!(headers.header("session_id"), None);
			assert_eq!(headers.header("x-client-request-id"), None);
			assert_eq!(headers.header("x-session-affinity"), None);
		});
	}

	#[test]
	fn sends_openrouter_session_affinity_header_by_default_for_built_in_openrouter_models() {
		run(|host| async move {
			let model = pi_ai::providers::all::get_builtin_model("openrouter", "auto").expect("built-in model");
			let (payload, headers) = capture_request(&host, session("session-openrouter"), model).await;
			assert_eq!(payload.get("session_id"), None);
			assert_eq!(payload.get("prompt_cache_key"), None);
			assert_eq!(headers.header("x-session-id"), Some("session-openrouter"));
			assert_eq!(headers.header("session_id"), None);
			assert_eq!(headers.header("x-client-request-id"), None);
			assert_eq!(headers.header("x-session-affinity"), None);
		});
	}

	#[test]
	fn omits_openrouter_session_affinity_data_when_disabled() {
		run(|host| async move {
			let model = create_model(|model| {
				model.base.provider = "openrouter".into();
				model.base.base_url = "https://openrouter.ai/api/v1".into();
				model.compat = compat(json!({ "sendSessionAffinityHeaders": false }));
			});
			let (payload, headers) = capture_request(&host, session("session-openrouter"), model).await;
			assert_eq!(payload.get("session_id"), None);
			assert_eq!(payload.get("prompt_cache_key"), None);
			assert_eq!(headers.header("x-session-id"), None);
		});
	}

	#[test]
	fn omits_session_affinity_headers_when_cache_retention_is_none() {
		run(|host| async move {
			let model = create_model(|model| {
				model.base.base_url = "https://proxy.example.com/v1".into();
				model.compat = compat(json!({ "sendSessionAffinityHeaders": true }));
			});
			let capture = CaptureOptions {
				cache_retention: Some(CacheRetention::None),
				..session("session-affinity")
			};
			let (_, headers) = capture_request(&host, capture, model).await;
			assert_eq!(headers.header("session_id"), None);
			assert_eq!(headers.header("x-client-request-id"), None);
			assert_eq!(headers.header("x-session-affinity"), None);
		});
	}

	#[test]
	fn lets_explicit_headers_override_generated_session_affinity_headers() {
		run(|host| async move {
			let model = create_model(|model| {
				model.base.base_url = "https://proxy.example.com/v1".into();
				model.compat = compat(json!({ "sendSessionAffinityHeaders": true }));
			});
			let capture = CaptureOptions {
				headers: Some(IndexMap::from([
					("session_id".to_string(), Some("override-session".to_string())),
					("x-client-request-id".to_string(), Some("override-request".to_string())),
					("x-session-affinity".to_string(), Some("override-affinity".to_string())),
				])),
				..session("session-affinity")
			};
			let (_, headers) = capture_request(&host, capture, model).await;
			assert_eq!(headers.header("session_id"), Some("override-session"));
			assert_eq!(headers.header("x-client-request-id"), Some("override-request"));
			assert_eq!(headers.header("x-session-affinity"), Some("override-affinity"));
		});
	}
}
