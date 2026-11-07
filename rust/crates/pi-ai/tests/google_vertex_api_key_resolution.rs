//! Port of `packages/ai/test/google-vertex-api-key-resolution.test.ts`.
//!
//! The TS mocks the `@google/genai` constructor and checks the config it receives. Here the real client
//! (`api/google_client.rs`) sends the request to the test host, and each check reads what that config decides in the
//! request: the ADC client sends `Authorization` from the host-served `authorized_user` credentials to the regional
//! `projects/<project>/locations/<location>` URL, the API key client sends `x-goog-api-key` to the express-mode URL
//! without project and location, `apiVersion` is the URL's version segment, `httpOptions.headers` are the request
//! headers, and `baseUrl` with `baseUrlResourceScope: COLLECTION` replaces the host and drops the project path.

use indexmap::IndexMap;
use pi_ai::api::google_vertex::{GoogleVertexOptions, stream};
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{
	Context, Message, Model, ProviderRequestOptions, StreamOptions, TranscriptContext, UserMessage, UserMessageContent,
};
use pi_ai::utils::identity::format_user_agent;
use pi_ai::utils::transcript::normalize_context;
use pi_host::HttpResponse;
use pi_host::js::json_stringify;
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::json;

const ADC_PATH: &str = "/adc/application_default_credentials.json";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REAL_API_KEY: &str = "AIzaSyExampleRealisticLookingApiKey123456";
const ADC_URL: &str = "https://us-central1-aiplatform.googleapis.com/v1/projects/test-project/locations/us-central1/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse";
const API_KEY_URL: &str = "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse";

fn model() -> Model {
	get_builtin_model("google-vertex", "gemini-3-flash-preview").expect("built-in model")
}

fn context() -> TranscriptContext {
	normalize_context(&Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("hello".into()),
			timestamp: pi_host::now_ms(),
			..UserMessage::default()
		})],
		..Context::default()
	})
}

/// Serves ADC and the mocked stream chunk.
fn serve(host: &TestHost) {
	host.set_env("GOOGLE_APPLICATION_CREDENTIALS", ADC_PATH);
	host.write_file(
		ADC_PATH,
		&json_stringify(&json!({
			"type": "authorized_user",
			"client_id": "client-id",
			"client_secret": "client-secret",
			"refresh_token": "refresh-token",
		})),
	);
	let chunk = json!({
		"responseId": "vertex-response-id",
		"candidates": [{ "content": { "parts": [{ "text": "ok" }] }, "finishReason": "STOP" }],
		"usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokenCount": 2 },
	});
	let body = format!("data: {}\r\n\r\n", json_stringify(&chunk));
	host.on_fetch(move |request| {
		let body = body.clone();
		Box::pin(async move {
			if request.url == TOKEN_URL {
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

fn options(api_key: Option<&str>, project_and_location: bool) -> GoogleVertexOptions {
	GoogleVertexOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: api_key.map(str::to_string),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		project: project_and_location.then(|| "test-project".into()),
		location: project_and_location.then(|| "us-central1".into()),
		..GoogleVertexOptions::default()
	}
}

/// Streams with `options` and returns the one generation request (token refreshes excluded).
async fn generation_request(host: &TestHost, model: &Model, options: GoogleVertexOptions) -> RecordedRequest {
	let result = stream(model, &context(), Some(options))
		.result()
		.await
		.expect("stream result");
	assert_eq!(result.error_message, None);
	let requests: Vec<RecordedRequest> = host
		.requests()
		.into_iter()
		.filter(|request| request.url != TOKEN_URL)
		.collect();
	assert_eq!(requests.len(), 1);
	requests[0].clone()
}

fn assert_adc_client(request: &RecordedRequest) {
	assert_eq!(request.header("authorization"), Some("Bearer test-access-token"));
	assert_eq!(request.header("x-goog-api-key"), None);
}

mod google_vertex_api_key_resolution {
	use super::*;

	#[test]
	fn falls_back_to_adc_when_options_api_key_is_a_placeholder_marker() {
		run(|host| async move {
			serve(&host);
			let request = generation_request(&host, &model(), options(Some("<authenticated>"), true)).await;

			assert_eq!(request.url, ADC_URL);
			assert_adc_client(&request);
		});
	}

	#[test]
	fn falls_back_to_adc_when_options_api_key_is_the_gcp_vertex_credentials_marker() {
		run(|host| async move {
			serve(&host);
			let request = generation_request(&host, &model(), options(Some("gcp-vertex-credentials"), true)).await;

			assert_eq!(request.url, ADC_URL);
			assert_adc_client(&request);
		});
	}

	#[test]
	fn falls_back_to_adc_when_google_cloud_api_key_is_a_placeholder_marker() {
		run(|host| async move {
			serve(&host);
			host.set_env("GOOGLE_CLOUD_API_KEY", "<authenticated>");
			let request = generation_request(&host, &model(), options(None, true)).await;

			assert_eq!(request.url, ADC_URL);
			assert_adc_client(&request);
		});
	}

	#[test]
	fn still_uses_the_api_key_client_for_real_api_keys() {
		run(|host| async move {
			serve(&host);
			let request = generation_request(&host, &model(), options(Some(REAL_API_KEY), false)).await;

			assert_eq!(request.url, API_KEY_URL);
			assert_eq!(request.header("x-goog-api-key"), Some(REAL_API_KEY));
			assert_eq!(request.header("authorization"), None);
			assert!(host.requests().iter().all(|request| request.url != TOKEN_URL));
		});
	}

	#[test]
	fn does_not_forward_generated_vertex_base_url_placeholders() {
		run(|host| async move {
			serve(&host);
			assert!(model().base_url.contains("{location}"));
			let request = generation_request(&host, &model(), options(None, true)).await;

			assert_eq!(request.url, ADC_URL);
			assert_eq!(request.header("User-Agent"), Some(format_user_agent(None).as_str()));
		});
	}

	#[test]
	fn lets_explicit_headers_override_the_default_user_agent() {
		run(|host| async move {
			serve(&host);
			let mut options = options(None, true);
			options.headers = Some(IndexMap::from([(
				"User-Agent".to_string(),
				Some("custom-agent".to_string()),
			)]));
			let request = generation_request(&host, &model(), options).await;

			assert_eq!(request.header("User-Agent"), Some("custom-agent"));
		});
	}

	fn custom_model(base_url: &str) -> Model {
		let mut model = model();
		model.base_url = base_url.into();
		model
	}

	#[test]
	fn forwards_custom_base_url_to_the_adc_client() {
		run(|host| async move {
			serve(&host);
			let request =
				generation_request(&host, &custom_model("https://proxy.example.com"), options(None, true)).await;

			assert_eq!(
				request.url,
				"https://proxy.example.com/v1/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse"
			);
			assert_adc_client(&request);
		});
	}

	#[test]
	fn forwards_custom_base_url_to_the_api_key_client() {
		run(|host| async move {
			serve(&host);
			let request = generation_request(
				&host,
				&custom_model("https://proxy.example.com"),
				options(Some(REAL_API_KEY), false),
			)
			.await;

			assert_eq!(
				request.url,
				"https://proxy.example.com/v1/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse"
			);
			assert_eq!(request.header("x-goog-api-key"), Some(REAL_API_KEY));
		});
	}

	#[test]
	fn does_not_append_api_version_when_custom_base_url_already_includes_one() {
		run(|host| async move {
			serve(&host);
			let request = generation_request(
				&host,
				&custom_model("https://proxy.example.com/v1/projects/test-project/locations/global"),
				options(None, true),
			)
			.await;

			assert_eq!(
				request.url,
				"https://proxy.example.com/v1/projects/test-project/locations/global/publishers/google/models/gemini-3-flash-preview:streamGenerateContent?alt=sse"
			);
		});
	}
}
