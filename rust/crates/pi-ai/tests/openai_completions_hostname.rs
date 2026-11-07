//! Port of `packages/ai/test/openai-completions-hostname.test.ts`.

mod openai_completions_support;

use std::cell::Cell;
use std::rc::Rc;

use openai_completions_support::{capture_payload, context, model, options, user_message};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::{CacheRetention, Model, StopReason};
use pi_host::testing::run;
use pi_host::{FetchFunction, PiError};
use serde_json::{Map, Value, json};

fn base_model() -> Model {
	let mut model = model("hostname-test", "custom", "https://example.test/v1");
	model.base.name = "Hostname test".into();
	model.context_window = 4096.0;
	model.max_tokens = 1024.0;
	model
}

async fn payload(base_url: &str, overrides: impl FnOnce(&mut Model)) -> Map<String, Value> {
	let mut model = base_model();
	overrides(&mut model);
	model.base.base_url = base_url.into();
	let calls = Rc::new(Cell::new(0));
	let counted = calls.clone();
	let fetch: FetchFunction = Rc::new(move |_| {
		counted.set(counted.get() + 1);
		Box::pin(async { Err(PiError::new("Unexpected provider request")) })
	});
	let (on_payload, captured) = capture_payload(Some("Payload captured before network request"));
	let mut request = options("test-key");
	request.fetch = Some(fetch);
	request.max_retries = Some(0.0);
	request.max_tokens = Some(16.0);
	request.cache_retention = Some(CacheRetention::Short);
	request.session_id = Some("hostname-session".into());
	request.on_payload = Some(on_payload);
	let result = stream(
		&model,
		&context(vec![user_message("Offline hostname test")]),
		Some(request),
	)
	.result()
	.await
	.unwrap();
	assert_eq!(result.stop_reason, StopReason::Error);
	assert!(
		result
			.error_message
			.as_deref()
			.unwrap()
			.contains("Payload captured before network request")
	);
	assert_eq!(calls.get(), 0);
	let captured = captured.borrow().clone().expect("No request payload captured");
	captured.as_object().cloned().unwrap()
}

mod openai_completions_endpoint_hostname_inference {
	use super::*;

	#[test]
	fn recognizes_the_real_endpoint_without_accepting_url_lookalikes() {
		run(|_host| async move {
			for host in [
				"api.cloudflare.com",
				"gateway.ai.cloudflare.com",
				"integrate.api.nvidia.com",
				"api.ant-ling.com",
				"api.deepseek.com",
				"api.together.ai",
				"api.together.xyz",
				"api.z.ai",
				"open.bigmodel.cn",
				"api.moonshot.ai",
				"api.moonshot.cn",
				"api.cerebras.ai",
				"chutes.ai",
				"opencode.ai",
				"api.x.ai",
			] {
				let official = payload(&format!("https://{host}/v1"), |_| {}).await;
				assert!(!official.contains_key("store"), "{host}");
				for lookalike in [
					format!("https://{host}.example.test/v1"),
					format!("https://example.test/v1/{host}"),
					format!("https://example.test/v1?endpoint={host}"),
					format!("https://example.test/v1#{host}"),
					format!("https://{host}@example.test/v1"),
				] {
					let generic = payload(&lookalike, |_| {}).await;
					assert_eq!(generic.get("store"), Some(&json!(false)), "{lookalike}");
					assert_eq!(generic.get("max_completion_tokens"), Some(&json!(16)), "{lookalike}");
					assert!(!generic.contains_key("max_tokens"), "{lookalike}");
				}
			}
		});
	}

	#[test]
	fn only_enables_the_official_openai_cache_key_for_its_hostname() {
		run(|_host| async move {
			assert_eq!(
				payload("https://api.openai.com/v1", |_| {})
					.await
					.get("prompt_cache_key"),
				Some(&json!("hostname-session"))
			);
			assert_eq!(
				payload("https://API.OPENAI.COM/v1", |_| {})
					.await
					.get("prompt_cache_key"),
				Some(&json!("hostname-session"))
			);
			for url in [
				"https://api.openai.com.example.test/v1",
				"https://example.test/api.openai.com/v1",
				"https://example.test/v1?api.openai.com",
				"https://api.openai.com@example.test/v1",
			] {
				assert_eq!(payload(url, |_| {}).await.get("prompt_cache_key"), None, "{url}");
			}
		});
	}

	#[test]
	fn keeps_openrouter_domain_inference_separate_from_lookalike_urls() {
		run(|_host| async move {
			for url in ["https://openrouter.ai/api/v1", "https://proxy.openrouter.ai/v1"] {
				let payload = payload(url, |model| model.reasoning = true).await;
				assert_eq!(payload.get("reasoning"), Some(&json!({ "effort": "none" })), "{url}");
			}
			for url in [
				"https://openrouter.ai.example.test/v1",
				"https://example.test/v1?openrouter.ai",
				"https://openrouter.ai@example.test/v1",
			] {
				let payload = payload(url, |model| model.reasoning = true).await;
				assert_eq!(payload.get("reasoning"), None, "{url}");
			}
		});
	}

	#[test]
	fn preserves_explicit_provider_identity_and_compatibility_overrides() {
		run(|_host| async move {
			let named = payload("https://example.test/v1", |model| model.base.provider = "nvidia".into()).await;
			assert!(!named.contains_key("store"));
			assert_eq!(named.get("max_tokens"), Some(&json!(16)));
			let explicit = payload("https://integrate.api.nvidia.com/v1", |model| {
				model.compat = json!({ "supportsStore": true, "maxTokensField": "max_completion_tokens" })
					.as_object()
					.cloned();
			})
			.await;
			assert_eq!(explicit.get("store"), Some(&json!(false)));
			assert_eq!(explicit.get("max_completion_tokens"), Some(&json!(16)));
		});
	}
}
