//! Port of `packages/ai/test/fetch-option.test.ts`.
//!
//! Every API adapter this file covers is owned by a w3 shard, so the file is ported as those adapters land. Ported:
//! "uses fetch for image generation", against the existing `PORT-STUB: w3-bedrock-misc`
//! `openrouter_images::generate_images`, which is `#[ignore = "needs w3-bedrock-misc: ..."]` until that adapter lands;
//! "rejects custom fetch for Google adapters instead of silently bypassing it" and "allows Google adapters to receive
//! globalThis.fetch explicitly" (w3-google-mistral).
//!
//! Not ported, because the adapter files are still empty and calling their predicted `stream_simple` would break the
//! crate's compile. They belong with the owning w3 shards:
//! - "passes fetch through streamSimple to the Anthropic SDK" (`api::anthropic_messages`, w3-anthropic).
//! - "passes fetch through streamSimple to OpenAI SDK adapters" (`api::openai_completions`, w3-openai-completions;
//!   `api::openai_responses` and `api::azure_openai_responses`, w3-openai-responses).
//! - "uses fetch for Mistral, Codex SSE, and pi-messages HTTP requests" (`api::mistral_conversations`,
//!   w3-google-mistral; `api::openai_codex_responses`, w3-openai-responses; `api::pi_messages`, w3-bedrock-misc).
//!
//! Differences from the TS:
//! - The stubbed global `fetch` is the test host's fetch, which records every request; "ambient fetch must not be
//!   called" is `host.requests()` staying empty. `expect(globalThis.fetch).toBe(fallback)` holds by construction: a
//!   request's `fetch` option cannot replace the host. `globalThis.fetch` passed as the option is `pi_host::global_fetch()`.

use std::cell::Cell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::openrouter_images::generate_images;
use pi_ai::api::{google_generative_ai, google_vertex};
use pi_ai::types::{
	BaseModel, Context, ImageModel, ImagesContext, ImagesOptions, Message, Model, ModelCost, ModelInputModality,
	ModelType, ProviderRequestOptions, SimpleStreamOptions, StreamOptions, TextContent, TranscriptContext, UserContent,
	UserMessage, UserMessageContent, image_api,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpResponse, global_fetch};

fn context() -> TranscriptContext {
	normalize_context(&Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("hello".into()),
			timestamp: 1.0,
			..UserMessage::default()
		})],
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

fn simple_options(fetch: FetchFunction) -> SimpleStreamOptions {
	SimpleStreamOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("test-key".into()),
				fetch: Some(fetch),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		..SimpleStreamOptions::default()
	}
}

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

mod fetch_stream_option {
	use super::*;

	#[test]
	fn rejects_custom_fetch_for_google_adapters_instead_of_silently_bypassing_it() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			let google = google_generative_ai::stream_simple(
				&create_model("google-generative-ai"),
				&context(),
				Some(simple_options(custom.clone())),
			)
			.unwrap()
			.result()
			.await
			.unwrap();
			let vertex =
				google_vertex::stream_simple(&create_model("google-vertex"), &context(), Some(simple_options(custom)))
					.unwrap()
					.result()
					.await
					.unwrap();

			assert!(
				google
					.error_message
					.unwrap()
					.contains("Custom fetch is not supported by the Google Generative AI adapter")
			);
			assert!(
				vertex
					.error_message
					.unwrap()
					.contains("Custom fetch is not supported by the Google Vertex adapter")
			);
			assert_eq!(calls.get(), 0);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn allows_google_adapters_to_receive_global_this_fetch_explicitly() {
		run(|host| async move {
			host.respond_with(
				401,
				&[("content-type", "application/json")],
				r#"{"error":{"message":"upstream rejected request"}}"#,
			);
			let result = google_generative_ai::stream_simple(
				&create_model("google-generative-ai"),
				&context(),
				Some(simple_options(global_fetch())),
			)
			.unwrap()
			.result()
			.await
			.unwrap();

			assert_eq!(host.requests().len(), 1);
			assert!(!result.error_message.unwrap().contains("Custom fetch is not supported"));
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
