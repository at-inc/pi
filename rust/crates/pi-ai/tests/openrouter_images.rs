//! Port of `packages/ai/test/openrouter-images.test.ts`.
//!
//! The TS suite `vi.mock`s the `openai` SDK; these tests mock at the HTTP level through `TestHost` and assert the
//! same params and results, plus the SDK request and error shapes the mock hid.

use pi_ai::images::generate_images;
use pi_ai::types::{
	BaseModel, ImageContent, ImageModel, ImagesContext, ImagesOptions, ImagesStopReason, ModelCost, ModelInputModality,
	ModelType, ProviderImagesOptions, ProviderRequestOptions, TextContent, UserContent,
};
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::testing::run;
use pi_host::{AbortController, HttpResponse};
use serde_json::{Value, json};

const RESPONSE: &str = r#"{"id":"img-1","usage":{"prompt_tokens":12,"completion_tokens":34,"prompt_tokens_details":{"cached_tokens":0}},"choices":[{"message":{"content":"Here is your image.","images":[{"image_url":"data:image/png;base64,ZmFrZS1wbmc="}]}}]}"#;

fn model(id: &str, name: &str, output: Vec<ModelInputModality>) -> ImageModel {
	ImageModel {
		base: BaseModel {
			id: id.into(),
			name: name.into(),
			api: "openrouter-images".into(),
			provider: "openrouter".into(),
			base_url: "https://openrouter.ai/api/v1".into(),
			input: vec![ModelInputModality::Text, ModelInputModality::Image],
			cost: ModelCost {
				input: 0.015,
				output: 0.03,
				cache_read: 0.0,
				cache_write: 0.0,
				..ModelCost::default()
			},
			..BaseModel::default()
		},
		r#type: ModelType::Image,
		output,
		extra: Default::default(),
	}
}

fn context() -> ImagesContext {
	ImagesContext {
		input: vec![UserContent::Text(TextContent {
			text: "Generate a dog".into(),
			..TextContent::default()
		})],
	}
}

fn options(options: ProviderRequestOptions<ImageModel>) -> Option<ProviderImagesOptions> {
	Some(ProviderImagesOptions {
		base: ImagesOptions {
			base: ProviderRequestOptions {
				api_key: Some("test".into()),
				..options
			},
			metadata: None,
		},
		extra: Default::default(),
	})
}

mod openrouter_images {
	use super::*;

	#[test]
	fn returns_text_plus_images_in_final_output() {
		run(|host| async move {
			host.respond_with(200, &[("content-type", "application/json")], RESPONSE);
			let mut model = model(
				"google/gemini-3.1-flash-image-preview",
				"Gemini 3.1 Flash Image Preview",
				vec![ModelInputModality::Text, ModelInputModality::Image],
			);
			model.base.headers = Some([("HTTP-Referer".to_string(), "https://example.com".to_string())].into());

			let output = generate_images(&model, &context(), options(ProviderRequestOptions::default()))
				.await
				.unwrap();
			assert_eq!(output.stop_reason, ImagesStopReason::Stop);
			assert_eq!(output.response_id.as_deref(), Some("img-1"));
			assert_eq!(
				output.output[0],
				UserContent::Text(TextContent {
					text: "Here is your image.".into(),
					..TextContent::default()
				})
			);
			assert_eq!(
				output.output[1],
				UserContent::Image(ImageContent {
					mime_type: "image/png".into(),
					data: "ZmFrZS1wbmc=".into(),
					..ImageContent::default()
				})
			);

			let requests = host.requests();
			let params = requests[0].json();
			assert_eq!(params["stream"], json!(false));
			assert_eq!(params["modalities"], json!(["image", "text"]));
			assert_eq!(
				params["messages"][0]["content"][0],
				json!({ "type": "text", "text": "Generate a dog" })
			);

			assert_eq!(requests[0].url, "https://openrouter.ai/api/v1/chat/completions");
			assert_eq!(requests[0].header("authorization"), Some("Bearer test"));
			assert_eq!(requests[0].header("http-referer"), Some("https://example.com"));
			assert_eq!(requests[0].header("content-type"), Some("application/json"));
			assert_eq!(requests[0].header("x-stainless-retry-count"), Some("0"));
			assert_eq!(requests[0].header("x-stainless-timeout"), None);
		});
	}

	#[test]
	fn passes_through_abort_signal_and_returns_aborted_result() {
		run(|host| async move {
			host.respond_with(200, &[("content-type", "application/json")], RESPONSE);
			let model = model(
				"black-forest-labs/flux.2-pro",
				"FLUX.2 Pro",
				vec![ModelInputModality::Image],
			);
			let controller = AbortController::new();
			controller.abort();

			let output = generate_images(
				&model,
				&context(),
				options(ProviderRequestOptions {
					signal: Some(controller.signal()),
					..ProviderRequestOptions::default()
				}),
			)
			.await
			.unwrap();
			assert_eq!(output.stop_reason, ImagesStopReason::Aborted);
			assert_eq!(output.error_message.as_deref(), Some("Request aborted"));
			// The SDK checks the signal before dispatch, so an aborted request never reaches fetch.
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn generate_images_resolves_the_final_assistant_images_result() {
		run(|host| async move {
			host.respond_with(200, &[("content-type", "application/json")], RESPONSE);
			let model = model(
				"black-forest-labs/flux.2-pro",
				"FLUX.2 Pro",
				vec![ModelInputModality::Image],
			);

			let output = generate_images(&model, &context(), options(ProviderRequestOptions::default()))
				.await
				.unwrap();
			assert!(output.output.iter().any(|item| matches!(item, UserContent::Image(_))));
			// Image-only models must not request text output.
			assert_eq!(host.requests()[0].json()["modalities"], json!(["image"]));
		});
	}
}

mod openai_sdk_behavior {
	use super::*;

	fn error_message(status: u16, headers: &[(&str, &str)], body: &'static str) -> Option<String> {
		run(|host| async move {
			host.respond_with(status, headers, body);
			let model = model("m", "m", vec![ModelInputModality::Image]);
			generate_images(&model, &context(), options(ProviderRequestOptions::default()))
				.await
				.unwrap()
				.error_message
		})
	}

	#[test]
	fn formats_status_errors_like_api_error_generate() {
		let json = [("content-type", "application/json")];
		assert_eq!(
			error_message(400, &json, r#"{"error":{"message":"bad","code":"x"}}"#).as_deref(),
			Some(r#"400: {"message":"bad","code":"x"}"#)
		);
		assert_eq!(error_message(400, &[], "plain text").as_deref(), Some("400 plain text"));
		assert_eq!(
			error_message(400, &[], "").as_deref(),
			Some("400 status code (no body)")
		);
		assert_eq!(
			error_message(400, &[], r#"{"error":"str"}"#).as_deref(),
			Some(r#"400 "str""#)
		);
		assert_eq!(
			error_message(400, &[], "5").as_deref(),
			Some("400 status code (no body)")
		);
		assert_eq!(error_message(400, &[], "null").as_deref(), Some("400 null"));
		assert_eq!(error_message(400, &[], "[1,2]").as_deref(), Some("400 [1,2]"));
	}

	#[test]
	fn reads_non_json_and_empty_bodies_like_default_parse_response() {
		assert_eq!(
			error_message(200, &[("content-type", "text/plain")], "not json").as_deref(),
			Some("Cannot read properties of undefined (reading '0')")
		);
		assert_eq!(
			error_message(200, &[("content-type", "application/json")], "").as_deref(),
			Some("Cannot read properties of undefined (reading 'id')")
		);
	}

	#[test]
	fn parses_cache_usage_and_rejects_fractional_timeouts() {
		run(|host| async move {
			host.respond_with(
				200,
				&[("content-type", "application/json")],
				r#"{"id":"r","choices":[{"message":{"content":"","images":[{"image_url":{"url":"data:image/png;base64,AAA"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":4,"cache_write_tokens":1}}}"#,
			);
			let mut model = model("m", "m", vec![ModelInputModality::Image]);
			model.base.cost = ModelCost {
				input: 1.0,
				output: 2.0,
				..ModelCost::default()
			};
			let output = generate_images(&model, &context(), options(ProviderRequestOptions::default()))
				.await
				.unwrap();
			assert_eq!(
				pi_host::js::json_stringify(&output.usage),
				r#"{"input":6,"output":5,"cacheRead":3,"cacheWrite":1,"totalTokens":15,"cost":{"input":0.000006,"output":0.000009999999999999999,"cacheRead":0,"cacheWrite":0,"total":0.000016,"totalFromProvider":false}}"#
			);
			assert_eq!(output.output.len(), 1);

			let output = generate_images(
				&model,
				&context(),
				options(ProviderRequestOptions {
					timeout_ms: Some(1.5),
					..ProviderRequestOptions::default()
				}),
			)
			.await
			.unwrap();
			assert_eq!(output.error_message.as_deref(), Some("timeout must be an integer"));
		});
	}

	#[test]
	fn reports_fetch_failures_as_connection_errors() {
		run(|host| async move {
			host.on_fetch(|_| Box::pin(async { Err(pi_host::PiError::type_error("fetch failed")) }));
			let model = model("m", "m", vec![ModelInputModality::Image]);
			let output = generate_images(&model, &context(), options(ProviderRequestOptions::default()))
				.await
				.unwrap();
			assert_eq!(output.error_message.as_deref(), Some("Connection error."));
		});
	}

	fn with_payload(payload: Value) -> Option<ProviderImagesOptions> {
		options(ProviderRequestOptions {
			on_payload: Some(Rc::new(move |_, _| {
				let payload = payload.clone();
				Box::pin(async move { Ok(Some(payload)) })
			})),
			..ProviderRequestOptions::default()
		})
	}

	#[test]
	fn sends_on_payload_replacements_through_create_and_build_body() {
		run(|host| async move {
			host.respond_with(200, &[("content-type", "application/json")], RESPONSE);
			let model = model("m", "m", vec![ModelInputModality::Image]);

			let output = generate_images(&model, &context(), with_payload(Value::Null))
				.await
				.unwrap();
			assert_eq!(output.stop_reason, ImagesStopReason::Error);
			assert_eq!(
				output.error_message.as_deref(),
				Some("Cannot read properties of null (reading 'stream')")
			);
			assert!(host.requests().is_empty());

			let output = generate_images(&model, &context(), with_payload(json!(false)))
				.await
				.unwrap();
			assert_eq!(output.stop_reason, ImagesStopReason::Stop);
			let request = host.requests().pop().unwrap();
			assert_eq!((request.body.as_deref(), request.header("content-type")), (None, None));

			let output = generate_images(&model, &context(), with_payload(json!({ "stream": true })))
				.await
				.unwrap();
			assert_eq!(output.stop_reason, ImagesStopReason::Error);
			assert_eq!(
				output.error_message.as_deref(),
				Some("Cannot read properties of undefined (reading '0')")
			);
			assert_eq!(
				host.requests().pop().unwrap().body.as_deref(),
				Some(r#"{"stream":true}"#)
			);
		});
	}

	#[test]
	fn reports_a_caller_abort_after_the_headers_as_a_user_abort() {
		run(|_| async move {
			let controller = AbortController::new();
			let abort = controller.clone();
			let fetch: pi_host::FetchFunction = Rc::new(move |_| {
				abort.abort();
				Box::pin(async {
					Ok(HttpResponse::from_bytes(
						200,
						IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
						RESPONSE,
					))
				})
			});
			let model = model("m", "m", vec![ModelInputModality::Image]);
			let output = generate_images(
				&model,
				&context(),
				options(ProviderRequestOptions {
					signal: Some(controller.signal()),
					fetch: Some(fetch),
					..ProviderRequestOptions::default()
				}),
			)
			.await
			.unwrap();
			assert_eq!(output.stop_reason, ImagesStopReason::Aborted);
			assert_eq!(output.error_message.as_deref(), Some("Request aborted"));
			assert!(output.output.is_empty());
		});
	}

	#[test]
	fn times_out_a_stalled_body_but_not_a_stalled_error_body() {
		run(|host| async move {
			let status = Rc::new(std::cell::Cell::new(200));
			let response_status = status.clone();
			host.on_fetch(move |_| {
				let status = response_status.get();
				Box::pin(async move {
					let body = if status == 200 {
						RESPONSE
					} else {
						r#"{"error":{"message":"slow"}}"#
					};
					let (head, tail) = body.split_at(10);
					let head = futures::stream::once(async move { Ok(head.to_string().into()) });
					let tail = futures::stream::once(async move {
						pi_host::sleep(2000.0, None).await?;
						Ok(tail.to_string().into())
					});
					Ok(HttpResponse {
						status,
						status_text: String::new(),
						url: String::new(),
						headers: IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
						body: futures::StreamExt::boxed_local(futures::StreamExt::chain(head, tail)),
					})
				})
			});
			let model = model("m", "m", vec![ModelInputModality::Image]);
			let timeout = || {
				options(ProviderRequestOptions {
					timeout_ms: Some(1000.0),
					..ProviderRequestOptions::default()
				})
			};

			let output = generate_images(&model, &context(), timeout()).await.unwrap();
			assert_eq!(output.error_message.as_deref(), Some("Request timed out."));

			status.set(400);
			let output = generate_images(&model, &context(), timeout()).await.unwrap();
			assert_eq!(output.error_message.as_deref(), Some(r#"400: {"message":"slow"}"#));
		});
	}

	#[test]
	fn builds_the_url_like_build_url() {
		run(|host| async move {
			host.respond_with(200, &[("content-type", "application/json")], RESPONSE);
			host.set_env("OPENAI_BASE_URL", "https://env.test/v1");
			let mut model = model("m", "m", vec![ModelInputModality::Image]);

			model.base.base_url = "https://x.test/api?b=1&2=x".into();
			generate_images(&model, &context(), options(ProviderRequestOptions::default()))
				.await
				.unwrap();
			model.base.base_url = String::new();
			generate_images(&model, &context(), options(ProviderRequestOptions::default()))
				.await
				.unwrap();

			let urls: Vec<String> = host.requests().into_iter().map(|request| request.url).collect();
			assert_eq!(
				urls,
				[
					"https://x.test/api?2=x%2Fchat%2Fcompletions&b=1",
					"https://api.openai.com/v1/chat/completions"
				]
			);
		});
	}
}
