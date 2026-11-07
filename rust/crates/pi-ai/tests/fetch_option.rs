//! Port of `packages/ai/test/fetch-option.test.ts`.
//!
//! Every case is ported in the TS order. Two depend on w3-bedrock-misc adapters that are still `PORT-STUB`s:
//! - "uses fetch for Mistral, Codex SSE, and pi-messages HTTP requests" covers `api::mistral_conversations` and
//!   `api::openai_codex_responses`; `api::pi_messages` joins it when w3-bedrock-misc lands, raising the count to 3.
//! - "uses fetch for image generation" runs against `openrouter_images::generate_images` and is
//!   `#[ignore = "needs w3-bedrock-misc: ..."]` until that adapter lands.
//!
//! Differences from the TS:
//! - The stubbed global `fetch` is the test host's fetch, which records every request; "ambient fetch must not be
//!   called" is `host.requests()` staying empty. `expect(globalThis.fetch).toBe(fallback)` holds by construction: a
//!   request's `fetch` option cannot replace the host. `globalThis.fetch` passed as the option is `pi_host::global_fetch()`.

use std::cell::Cell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::anthropic_messages;
use pi_ai::api::openrouter_images::generate_images;
use pi_ai::api::{
	azure_openai_responses, google_generative_ai, google_vertex, mistral_conversations, openai_codex_responses,
	openai_completions, openai_responses,
};
use pi_ai::types::{
	BaseModel, Context, ImageModel, ImagesContext, ImagesOptions, Message, Model, ModelCost, ModelInputModality,
	ModelType, ProviderRequestOptions, SimpleStreamOptions, StreamOptions, TextContent, TranscriptContext, Transport,
	UserContent, UserMessage, image_api,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpResponse, global_fetch};

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

/// `createModel(api)`.
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
	fn passes_fetch_through_stream_simple_to_open_ai_sdk_adapters() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			let max_retries = |options: &mut SimpleStreamOptions| options.max_retries = Some(0.0);
			openai_completions::stream_simple(
				&create_model("openai-completions"),
				&context(),
				Some(simple_options("test-key", &custom, max_retries)),
			)
			.expect("stream starts")
			.result()
			.await;
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

			assert_eq!(calls.get(), 3);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn uses_fetch_for_mistral_codex_sse_and_pi_messages_http_requests() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			mistral_conversations::stream_simple(
				&create_model("mistral-conversations"),
				&context(),
				Some(simple_options("test-key", &custom, |_| {})),
			)
			.expect("stream starts")
			.result()
			.await;
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

			assert_eq!(calls.get(), 2);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn rejects_custom_fetch_for_google_adapters_instead_of_silently_bypassing_it() {
		run(|host| async move {
			let (custom, calls) = custom_fetch();
			let google = google_generative_ai::stream_simple(
				&create_model("google-generative-ai"),
				&context(),
				Some(simple_options("test-key", &custom, |_| {})),
			)
			.unwrap()
			.result()
			.await
			.unwrap();
			let vertex = google_vertex::stream_simple(
				&create_model("google-vertex"),
				&context(),
				Some(simple_options("test-key", &custom, |_| {})),
			)
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
				Some(simple_options("test-key", &global_fetch(), |_| {})),
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
