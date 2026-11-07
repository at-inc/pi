//! Port of `packages/ai/test/openai-codex-images-options.test.ts`.
//!
//! Differences from the TS: the `vi.fn` fetch records each request and the assertions the TS makes inside it run on
//! the recorded request afterwards; `mockImplementation` is a switch the fetch reads.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use base64::Engine;
use indexmap::IndexMap;
use pi_ai::api::openai_codex_images::generate_images;
use pi_ai::types::{
	ImageContent, ImageModel, ImagesContext, ImagesOptions, ImagesStopReason, ProviderRequestOptions, UserContent,
};
use pi_host::js::json_stringify;
use pi_host::{FetchFunction, HttpRequest, HttpResponse};
use pi_telemetry::NOOP_TELEMETRY_CONTEXT;
use serde_json::{Value, json};

fn json_response(body: Value, status: u16) -> HttpResponse {
	HttpResponse::from_bytes(
		status,
		IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
		json_stringify(&body),
	)
}

mod codex_image_request_options {
	use super::*;

	#[test]
	fn accepts_v1_4_telemetry_and_forwards_client_identity_without_enabling_retries_or_caching() {
		pi_host::testing::run(|_host| async move {
			let model: ImageModel = serde_json::from_value(json!({
				"type": "image",
				"id": "chatgpt-image-generation",
				"name": "ChatGPT Image Generation",
				"api": "openai-codex-images",
				"provider": "openai-codex",
				"baseUrl": "https://chatgpt.com/backend-api",
				"input": ["text", "image"],
				"output": ["image"],
				"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
			}))
			.unwrap();
			let payload = base64::engine::general_purpose::STANDARD.encode(json_stringify(
				&json!({ "https://api.openai.com/auth": { "chatgpt_account_id": "acc_test" } }),
			));
			let requests: Rc<RefCell<Vec<HttpRequest>>> = Rc::default();
			let failing = Rc::new(Cell::new(false));
			let (recorded, fail) = (requests.clone(), failing.clone());
			let fetch: FetchFunction = Rc::new(move |request| {
				recorded.borrow_mut().push(request);
				let response = if fail.get() {
					json_response(json!({ "error": { "message": "unavailable" } }), 503)
				} else {
					json_response(json!({ "created": 1, "data": [{ "b64_json": "aW1hZ2U=" }] }), 200)
				};
				Box::pin(async move { Ok(response) })
			});
			let options = ImagesOptions {
				base: ProviderRequestOptions {
					api_key: Some(format!("header.{payload}.signature")),
					fetch: Some(fetch),
					telemetry: Some(NOOP_TELEMETRY_CONTEXT.with(Rc::clone)),
					identity: Some(
						serde_json::from_value(json!({ "userAgent": { "custom": "image-client" } })).unwrap(),
					),
					max_retries: Some(4.0),
					..ProviderRequestOptions::default()
				},
				..ImagesOptions::default()
			};
			let context: ImagesContext =
				serde_json::from_value(json!({ "input": [{ "type": "text", "text": "Draw a circle" }] })).unwrap();
			let result = generate_images(&model, &context, Some(options.clone())).await.unwrap();

			{
				let requests = requests.borrow();
				assert_eq!(requests.len(), 1);
				let request = &requests[0];
				assert_eq!(request.url, "https://chatgpt.com/backend-api/codex/images/generations");
				assert_eq!(request.header("user-agent"), Some("image-client"));
				let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
				assert_eq!(
					body,
					json!({ "model": "gpt-image-2", "prompt": "Draw a circle", "n": 1 })
				);
			}
			assert_eq!(result.stop_reason, ImagesStopReason::Stop);
			assert_eq!(
				result.output,
				[UserContent::Image(ImageContent {
					data: "aW1hZ2U=".into(),
					mime_type: "image/png".into(),
					..ImageContent::default()
				})]
			);
			requests.borrow_mut().clear();
			failing.set(true);
			let failed = generate_images(&model, &context, Some(options)).await.unwrap();
			assert_eq!(failed.stop_reason, ImagesStopReason::Error);
			assert_eq!(requests.borrow().len(), 1);
		});
	}
}
