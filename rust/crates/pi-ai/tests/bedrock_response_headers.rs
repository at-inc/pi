//! Port of `packages/ai/test/bedrock-response-headers.test.ts`.
//!
//! The TS starts a `node:http` server that answers with event stream headers and an empty body; the Rust test answers
//! the same response through the test host's fetch, so the model's `baseUrl` only has to parse.

mod bedrock_support;

use std::cell::RefCell;
use std::rc::Rc;

use bedrock_support::{builtin_model, serve, stream_response};
use indexmap::IndexMap;
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream};
use pi_ai::types::{CacheRetention, Model, ProviderRequestOptions, ProviderResponse, StopReason, StreamOptions};
use pi_host::testing::run;
use serde_json::json;

const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";

/// `startBedrockResponseServer()`: every request gets a 200 event stream response with no events.
fn start_bedrock_response_server(host: &pi_host::testing::TestHost) -> String {
	serve(
		host,
		vec![stream_response(
			&[],
			&[
				("x-bifrost-provider", "bedrock"),
				("x-bifrost-resolved-model", MODEL_ID),
				("x-amzn-requestid", "req-123"),
			],
		)],
	);
	"http://127.0.0.1:45124".into()
}

mod bedrock_response_headers {
	use super::*;

	#[test]
	fn forwards_raw_smithy_response_headers_to_on_response() {
		run(|host| async move {
			let mut model = builtin_model(MODEL_ID);
			model.base_url = start_bedrock_response_server(&host);
			let responses: Rc<RefCell<Vec<ProviderResponse>>> = Rc::default();
			let recorded = responses.clone();

			let options = BedrockOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						env: Some(IndexMap::from([
							("AWS_BEDROCK_FORCE_HTTP1".to_string(), "1".to_string()),
							("AWS_BEDROCK_SKIP_AUTH".to_string(), "1".to_string()),
						])),
						on_response: Some(Rc::new(move |response: ProviderResponse, _model: &Model| {
							recorded.borrow_mut().push(response);
							Box::pin(async { Ok(()) })
						})),
						..ProviderRequestOptions::default()
					},
					cache_retention: Some(CacheRetention::None),
					..StreamOptions::default()
				},
				..BedrockOptions::default()
			};
			let context = bedrock_support::context(
				json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }),
			);
			let result = stream(&model, &context, Some(options))
				.result()
				.await
				.expect("a final message");

			// The fake server intentionally returns an empty event stream; this assertion
			// documents that the header callback still fires before stream consumption.
			assert_eq!(result.stop_reason, StopReason::Error);
			let responses = responses.borrow();
			assert_eq!(responses.len(), 1);
			assert_eq!(responses[0].status, 200);
			assert_eq!(
				responses[0].headers.get("x-amzn-requestid").map(String::as_str),
				Some("req-123")
			);
			assert_eq!(
				responses[0].headers.get("x-bifrost-provider").map(String::as_str),
				Some("bedrock")
			);
			assert_eq!(
				responses[0].headers.get("x-bifrost-resolved-model").map(String::as_str),
				Some(MODEL_ID)
			);
		});
	}
}
