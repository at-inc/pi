//! Port of `packages/ai/test/fetch-option.test.ts`.
//!
//! Most API adapters this file covers are owned by w3 shards. "uses fetch for image generation" is ported against the
//! existing `PORT-STUB: w3-bedrock-misc` `openrouter_images::generate_images`; it is
//! `#[ignore = "needs w3-bedrock-misc: ..."]` until that adapter lands.
//!
//! Ported in part, with the adapters w3-openai-responses ports; the owning shards add theirs to the same lists:
//! - "passes fetch through streamSimple to OpenAI SDK adapters": `api::openai_responses` and
//!   `api::azure_openai_responses`. Missing: `api::openai_completions` (w3-openai-completions).
//! - "uses fetch for Mistral, Codex SSE, and pi-messages HTTP requests": `api::openai_codex_responses` over SSE.
//!   Missing: `api::mistral_conversations` (w3-google-mistral) and `api::pi_messages` (w3-bedrock-misc).
//!
//! Not ported, because the adapter files are still empty and calling their predicted `stream_simple` would break the
//! crate's compile. They belong with the owning w3 shards:
//! - "passes fetch through streamSimple to the Anthropic SDK" (`api::anthropic_messages`, w3-anthropic).
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
use pi_ai::api::openrouter_images::generate_images;
use pi_ai::api::{azure_openai_responses, openai_codex_responses, openai_responses};
use pi_ai::types::{
	BaseModel, Context, ImageModel, ImagesContext, ImagesOptions, Message, Model, ModelCost, ModelInputModality,
	ModelType, ProviderRequestOptions, SimpleStreamOptions, TextContent, TranscriptContext, Transport, UserContent,
	UserMessage, image_api,
};
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

fn context() -> TranscriptContext {
	normalize_context(&Context {
		messages: vec![Message::User(
			serde_json::from_value::<UserMessage>(
				serde_json::json!({ "role": "user", "content": "hello", "timestamp": 1 }),
			)
			.expect("user message"),
		)],
		..Context::default()
	})
}

fn create_model(api: &str) -> Model {
	Model {
		base: BaseModel {
			id: "test-model".into(),
			name: "Test Model".into(),
			api: api.into(),
			provider: "test-provider".into(),
			base_url: "https://upstream.test/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 10_000.0,
		max_tokens: 1_000.0,
		..Model::default()
	}
}

fn simple_options(
	api_key: &str,
	fetch: &FetchFunction,
	configure: impl FnOnce(&mut SimpleStreamOptions),
) -> SimpleStreamOptions {
	let mut options = SimpleStreamOptions {
		tool_choice: None,
		..SimpleStreamOptions::default()
	};
	configure(&mut options);
	options.api_key = Some(api_key.into());
	options.fetch = Some(fetch.clone());
	options
}

mod fetch_stream_option {
	use super::*;

	#[test]
	fn passes_fetch_through_stream_simple_to_open_ai_sdk_adapters() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			let max_retries = |options: &mut SimpleStreamOptions| options.max_retries = Some(0.0);
			openai_responses::stream_simple(
				&create_model("openai-responses"),
				&context(),
				Some(simple_options("test-key", &custom, max_retries)),
			)
			.expect("stream starts")
			.result()
			.await;
			azure_openai_responses::stream_simple(
				&create_model("azure-openai-responses"),
				&context(),
				Some(simple_options("test-key", &custom, max_retries)),
			)
			.expect("stream starts")
			.result()
			.await;

			assert_eq!(calls.get(), 2);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn uses_fetch_for_mistral_codex_sse_and_pi_messages_http_requests() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			// `header.${btoa(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "account" } }))}.signature`
			let api_key = "header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjb3VudCJ9fQ==.signature";
			openai_codex_responses::stream_simple(
				&create_model("openai-codex-responses"),
				&context(),
				Some(simple_options(api_key, &custom, |options| {
					options.transport = Some(Transport::Sse);
					options.max_retries = Some(0.0);
				})),
			)
			.expect("stream starts")
			.result()
			.await;

			assert_eq!(calls.get(), 1);
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
