//! Port of `packages/ai/test/fetch-option.test.ts`.
//!
//! "passes fetch through streamSimple to the Anthropic SDK" is ported against `api::anthropic_messages`. Every other
//! API adapter this file covers is `PORT-STATUS: pending` and owned by a w3 shard, so the rest is mostly not
//! ported yet. Only "uses fetch for image generation" is ported, against the existing `PORT-STUB: w3-bedrock-misc`
//! `openrouter_images::generate_images`; it is `#[ignore = "needs w3-bedrock-misc: ..."]` until that adapter lands.
//!
//! Not ported, because the adapter files are still empty and calling their predicted `stream_simple` would break the
//! crate's compile. They belong with the owning w3 shards:
//! - "passes fetch through streamSimple to OpenAI SDK adapters" (`api::openai_completions`, w3-openai-completions;
//!   `api::openai_responses` and `api::azure_openai_responses`, w3-openai-responses).
//! - "uses fetch for Mistral, Codex SSE, and pi-messages HTTP requests" (`api::mistral_conversations`,
//!   w3-google-mistral; `api::openai_codex_responses`, w3-openai-responses; `api::pi_messages`, w3-bedrock-misc).
//! - "rejects custom fetch for Google adapters instead of silently bypassing it" (`api::google_generative_ai` and
//!   `api::google_vertex`, w3-google-mistral).
//! - "allows Google adapters to receive globalThis.fetch explicitly" (`api::google_generative_ai`, w3-google-mistral).
//!
//! Differences from the TS:
//! - The stubbed global `fetch` is the test host's fetch, which records every request; "ambient fetch must not be
//!   called" is `host.requests()` staying empty. `expect(globalThis.fetch).toBe(fallback)` holds by construction: a
//!   request's `fetch` option cannot replace the host.

use std::cell::Cell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::anthropic_messages;
use pi_ai::api::openrouter_images::generate_images;
use pi_ai::types::{
	BaseModel, ImageModel, ImagesContext, ImagesOptions, ModelCost, ModelInputModality, ModelType,
	ProviderRequestOptions, TextContent, UserContent, image_api,
};
use pi_ai::types::{Model, SimpleStreamOptions, StreamOptions};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpResponse};

/// `mockFetches()`'s `custom`: answers every request with a 401 JSON error and counts the calls.
fn custom_fetch() -> (FetchFunction, Rc<Cell<usize>>) {
	let calls = Rc::new(Cell::new(0));
	let counted = calls.clone();
	let fetch: FetchFunction = Rc::new(move |_| {
		counted.set(counted.get() + 1);
		Box::pin(async {
			Ok(HttpResponse::from_bytes(
				401,
				IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
				r#"{"error":{"message":"upstream rejected request"}}"#.as_bytes().to_vec(),
			))
		})
	});
	(fetch, calls)
}

/// `createModel(api)`.
fn create_model(api: &str) -> Model {
	serde_json::from_value(serde_json::json!({
		"id": "test-model",
		"name": "Test Model",
		"api": api,
		"provider": "test-provider",
		"baseUrl": "https://upstream.test/v1",
		"reasoning": false,
		"input": ["text"],
		"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
		"contextWindow": 10_000,
		"maxTokens": 1_000,
	}))
	.unwrap()
}

mod fetch_stream_option {
	use super::*;

	#[test]
	fn passes_fetch_through_stream_simple_to_the_anthropic_sdk() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			let context = normalize_context(
				&serde_json::from_value(serde_json::json!({
					"messages": [{ "role": "user", "content": "hello", "timestamp": 1 }],
				}))
				.unwrap(),
			);
			let options = SimpleStreamOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some("test-key".into()),
						fetch: Some(custom),
						max_retries: Some(0.0),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				},
				..SimpleStreamOptions::default()
			};
			anthropic_messages::stream_simple(&create_model("anthropic-messages"), &context, Some(options))
				.unwrap()
				.result()
				.await;
			assert!(calls.get() > 0);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	#[ignore = "needs w3-bedrock-misc: openrouter_images::generate_images"]
	fn uses_fetch_for_image_generation() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			let model = ImageModel {
				base: BaseModel {
					id: "test-model".into(),
					name: "Test Model".into(),
					api: image_api::OPENROUTER_IMAGES.into(),
					provider: "openrouter".into(),
					base_url: "https://upstream.test/v1".into(),
					input: vec![ModelInputModality::Text],
					cost: ModelCost::default(),
					..BaseModel::default()
				},
				r#type: ModelType::Image,
				output: vec![ModelInputModality::Image],
				extra: Default::default(),
			};
			let _ = generate_images(
				&model,
				&ImagesContext {
					input: vec![UserContent::Text(TextContent {
						text: "draw".into(),
						..TextContent::default()
					})],
				},
				Some(ImagesOptions {
					base: ProviderRequestOptions {
						api_key: Some("test-key".into()),
						fetch: Some(custom),
						max_retries: Some(0.0),
						..ProviderRequestOptions::default()
					},
					metadata: None,
				}),
			)
			.await;
			assert!(calls.get() > 0);
			assert!(host.requests().is_empty());
		});
	}
}
