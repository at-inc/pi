//! Port of `packages/ai/test/google-raw-stop-reason.test.ts`.
//!
//! Differences from the TS:
//! - The TS mocks `@google/genai`. Here the real client (`api/google_client.rs`) talks to the test host, which serves
//!   the mocked chunks as the Gemini SSE stream. Vertex runs on ADC, so the host also serves an `authorized_user`
//!   credentials file and its token refresh.
//! - "Google Generative AI user agent" reads the `User-Agent` of the request the client sends instead of the
//!   `httpOptions.headers` the mocked constructor received.
//! - The chunks `onProviderStreamEvent` receives are the SDK's `GenerateContentResponse` values, which carry
//!   `sdkHttpResponse` besides the wire fields; the TS mock yields the bare objects. The check compares the wire fields.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::{google_generative_ai, google_vertex};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{
	AssistantContent, AssistantMessage, Context, Message, Model, OnProviderStreamEvent, ProviderRequestOptions,
	StopReason, StreamOptions, TextContent, UserMessage, UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::identity::format_user_agent;
use pi_ai::utils::transcript::normalize_context;
use pi_host::HttpResponse;
use pi_host::js::json_stringify;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

const ADC_PATH: &str = "/adc/application_default_credentials.json";

fn context() -> pi_ai::types::TranscriptContext {
	normalize_context(&Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("hello".into()),
			timestamp: pi_host::now_ms(),
			..UserMessage::default()
		})],
		..Context::default()
	})
}

fn google_model() -> Model {
	get_builtin_model("google", "gemini-2.5-flash").expect("built-in model")
}

fn vertex_model() -> Model {
	get_builtin_model("google-vertex", "gemini-3-flash-preview").expect("built-in model")
}

/// `googleGenAiMock`'s default chunk.
fn default_chunk(finish_reason: &str, include_function_call: bool) -> Value {
	let mut candidate = json!({ "finishReason": finish_reason });
	if include_function_call {
		candidate["content"] = json!({
			"parts": [{ "functionCall": { "id": "call-1", "name": "echo", "args": { "value": "truncated" } } }],
		});
	}
	json!({
		"responseId": "google-response-id",
		"candidates": [candidate],
		"usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 0, "totalTokenCount": 1 },
	})
}

/// Serves `chunks` as the Gemini SSE stream, and ADC for Vertex.
fn serve(host: &TestHost, chunks: Vec<Value>) {
	host.set_env("GOOGLE_APPLICATION_CREDENTIALS", ADC_PATH);
	host.add_file_with_content(
		ADC_PATH,
		&json_stringify(&json!({
			"type": "authorized_user",
			"client_id": "client-id",
			"client_secret": "client-secret",
			"refresh_token": "refresh-token",
		})),
	);
	let body: String = chunks
		.iter()
		.map(|chunk| format!("data: {}\r\n\r\n", json_stringify(chunk)))
		.collect();
	host.on_fetch(move |request| {
		let body = body.clone();
		Box::pin(async move {
			if request.url == "https://oauth2.googleapis.com/token" {
				return Ok(HttpResponse::from_bytes(
					200,
					IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
					r#"{"access_token":"test-access-token","expires_in":3600,"token_type":"Bearer"}"#,
				));
			}
			Ok(HttpResponse::from_bytes(
				200,
				IndexMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
				body,
			))
		})
	});
}

fn google_options(on_provider_stream_event: Option<OnProviderStreamEvent>) -> google_generative_ai::GoogleOptions {
	google_generative_ai::GoogleOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("test-api-key".into()),
				..ProviderRequestOptions::default()
			},
			on_provider_stream_event,
			..StreamOptions::default()
		},
		..google_generative_ai::GoogleOptions::default()
	}
}

fn vertex_options(on_provider_stream_event: Option<OnProviderStreamEvent>) -> google_vertex::GoogleVertexOptions {
	google_vertex::GoogleVertexOptions {
		base: StreamOptions {
			on_provider_stream_event,
			..StreamOptions::default()
		},
		project: Some("test-project".into()),
		location: Some("us-central1".into()),
		..google_vertex::GoogleVertexOptions::default()
	}
}

#[derive(Clone, Copy, Debug)]
enum Adapter {
	GoogleGenerativeAI,
	GoogleVertex,
}

const ADAPTERS: [Adapter; 2] = [Adapter::GoogleGenerativeAI, Adapter::GoogleVertex];

impl Adapter {
	fn model(self) -> Model {
		match self {
			Adapter::GoogleGenerativeAI => google_model(),
			Adapter::GoogleVertex => vertex_model(),
		}
	}

	fn create_stream(self, on_provider_stream_event: Option<OnProviderStreamEvent>) -> AssistantMessageEventStream {
		match self {
			Adapter::GoogleGenerativeAI => google_generative_ai::stream(
				&self.model(),
				&context(),
				Some(google_options(on_provider_stream_event)),
			),
			Adapter::GoogleVertex => google_vertex::stream(
				&self.model(),
				&context(),
				Some(vertex_options(on_provider_stream_event)),
			),
		}
	}
}

async fn result(stream: AssistantMessageEventStream) -> AssistantMessage {
	stream.result().await.expect("stream result")
}

fn has_tool_call(message: &AssistantMessage) -> bool {
	message
		.content
		.iter()
		.any(|block| matches!(block, AssistantContent::ToolCall(_)))
}

async fn capture_google_user_agent(host: &TestHost, headers: Option<&[(&str, &str)]>) -> Option<String> {
	serve(host, vec![default_chunk("STOP", false)]);
	let mut options = google_options(None);
	options.headers = headers.map(|headers| {
		headers
			.iter()
			.map(|(name, value)| ((*name).to_string(), Some((*value).to_string())))
			.collect()
	});
	result(google_generative_ai::stream(&google_model(), &context(), Some(options))).await;
	let requests = host.requests();
	assert_eq!(requests.len(), 1);
	requests[0].header("User-Agent").map(str::to_string)
}

mod google_raw_stop_reasons {
	use super::*;

	#[test]
	fn preserves_raw_gemini_finish_reasons_for_google_generative_ai_errors() {
		run(|host| async move {
			serve(&host, vec![default_chunk("MALFORMED_FUNCTION_CALL", false)]);
			let message = result(Adapter::GoogleGenerativeAI.create_stream(None)).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("MALFORMED_FUNCTION_CALL"));
			assert_eq!(
				message.error_message.as_deref(),
				Some("Provider stopped with: MALFORMED_FUNCTION_CALL")
			);
		});
	}

	#[test]
	fn preserves_raw_gemini_finish_reasons_for_google_vertex_errors() {
		run(|host| async move {
			serve(&host, vec![default_chunk("SAFETY", false)]);
			let message = result(Adapter::GoogleVertex.create_stream(None)).await;

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("SAFETY"));
			assert_eq!(message.error_message.as_deref(), Some("Provider stopped with: SAFETY"));
		});
	}

	#[test]
	fn preserves_max_tokens_with_a_tool_call_as_length() {
		for adapter in ADAPTERS {
			run(|host| async move {
				serve(&host, vec![default_chunk("MAX_TOKENS", true)]);
				let message = result(adapter.create_stream(None)).await;

				assert_eq!(message.stop_reason, StopReason::Length, "{adapter:?}");
				assert_eq!(message.raw_stop_reason.as_deref(), Some("MAX_TOKENS"), "{adapter:?}");
				assert!(has_tool_call(&message), "{adapter:?}");
			});
		}
	}

	#[test]
	fn maps_stop_with_a_tool_call_to_tool_use() {
		for adapter in ADAPTERS {
			run(|host| async move {
				serve(&host, vec![default_chunk("STOP", true)]);
				let message = result(adapter.create_stream(None)).await;

				assert_eq!(message.stop_reason, StopReason::ToolUse, "{adapter:?}");
				assert_eq!(message.raw_stop_reason.as_deref(), Some("STOP"), "{adapter:?}");
				assert!(has_tool_call(&message), "{adapter:?}");
			});
		}
	}
}

mod google_provider_stream_events {
	use super::*;

	#[test]
	fn forwards_each_sdk_chunk_in_order_before_normalizing_it() {
		for adapter in ADAPTERS {
			run(|host| async move {
				let stream_chunks = vec![
					json!({
						"responseId": "resp_google",
						"candidates": [{ "content": { "parts": [{ "text": "hello" }] } }],
					}),
					json!({
						"candidates": [{ "finishReason": "STOP" }],
						"usageMetadata": { "promptTokenCount": 2, "candidatesTokenCount": 1, "totalTokenCount": 3 },
					}),
				];
				serve(&host, stream_chunks.clone());
				let received: Rc<RefCell<Vec<Value>>> = Rc::default();
				let event_models: Rc<RefCell<Vec<Model>>> = Rc::default();
				let (received_in, event_models_in) = (received.clone(), event_models.clone());
				let on_provider_stream_event: OnProviderStreamEvent = Rc::new(move |chunk, event_model| {
					let mut chunk = chunk.clone();
					chunk
						.as_object_mut()
						.expect("chunk is an object")
						.shift_remove("sdkHttpResponse");
					let (received, event_models, event_model) =
						(received_in.clone(), event_models_in.clone(), event_model.clone());
					Box::pin(async move {
						futures::future::ready(()).await;
						received.borrow_mut().push(chunk);
						event_models.borrow_mut().push(event_model);
						Ok(())
					})
				});
				let result = result(adapter.create_stream(Some(on_provider_stream_event))).await;

				assert_eq!(*received.borrow(), stream_chunks, "{adapter:?}");
				assert_eq!(
					*event_models.borrow(),
					vec![adapter.model(), adapter.model()],
					"{adapter:?}"
				);
				assert_eq!(result.stop_reason, StopReason::Stop, "{adapter:?}");
				assert_eq!(result.response_id.as_deref(), Some("resp_google"), "{adapter:?}");
				assert_eq!(
					result.content,
					vec![AssistantContent::Text(TextContent {
						text: "hello".into(),
						..TextContent::default()
					})],
					"{adapter:?}"
				);
			});
		}
	}
}

mod google_generative_ai_user_agent {
	use super::*;

	#[test]
	fn uses_pis_user_agent_by_default() {
		run(|host| async move {
			assert_eq!(
				capture_google_user_agent(&host, None).await,
				Some(format_user_agent(None))
			);
		});
	}

	#[test]
	fn lets_explicit_headers_override_the_default_user_agent() {
		run(|host| async move {
			assert_eq!(
				capture_google_user_agent(&host, Some(&[("User-Agent", "custom-agent")])).await,
				Some("custom-agent".into())
			);
		});
	}
}
