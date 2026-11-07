//! Port of `packages/ai/test/openai-codex-images.test.ts`.
//!
//! Differences from the TS:
//! - The `vi.fn` fetch mocks record each request; the assertions the TS makes inside the mock run on the recorded
//!   request after `generateImages` resolves. `init?.signal instanceof AbortSignal` is the request carrying a signal.
//! - `it.each` cases run one after another in one `#[test]`, each case in its own test host.
//! - `new OpenAI.APIConnectionTimeoutError()` is the error `openai_client.rs` throws for it: `Request timed out.` with no
//!   status and `headers: null` data.
//! - The test host's timers are virtual, so "before the image timeout signal fires" is the virtual clock staying
//!   short of the 60s timeout.
//! - `expect(result).toMatchObject(...)` compares the serialized result with `toMatchObject` semantics.
//! - `getImageModel` and `generateImages` from `compat.ts` are the `pi_ai::compat` re-exports.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use base64::Engine;
use bytes::Bytes;
use futures::StreamExt;
use indexmap::IndexMap;
use pi_ai::api::openai_codex_images::generate_images;
use pi_ai::auth::credential_store::InMemoryCredentialStore;
use pi_ai::auth::types::{Credential, CredentialStore, OAuthCredential};
use pi_ai::models::{CreateModelsOptions, ModelsImagesOptions, create_models};
use pi_ai::providers::all::{builtin_images_providers, builtin_models, get_builtin_image_model};
use pi_ai::providers::openai_codex_images::openai_codex_images_provider;
use pi_ai::types::{
	AnyModel, AssistantImages, ImageModel, ImagesContext, ImagesOptions, ModelType, OnPayload, OnResponse,
	ProviderImagesOptions, ProviderResponse,
};
use pi_host::js::json_stringify;
use pi_host::{AbortController, FetchFunction, HttpRequest, HttpResponse, LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};

fn model() -> ImageModel {
	serde_json::from_value(json!({
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
	.expect("valid image model")
}

const IMAGE_DATA: &str = "aW1hZ2U=";

fn context() -> ImagesContext {
	serde_json::from_value(json!({ "input": [{ "type": "text", "text": "Draw a fox" }] })).expect("valid context")
}

fn mock_token() -> String {
	let payload = base64::engine::general_purpose::STANDARD.encode(json_stringify(
		&json!({ "https://api.openai.com/auth": { "chatgpt_account_id": "acc_test" } }),
	));
	format!("header.{payload}.signature")
}

/// `Response.json(body, { status, headers })`.
fn json_response(body: Value, status: u16, headers: &[(&str, &str)]) -> HttpResponse {
	let mut response_headers = IndexMap::from([("content-type".to_string(), "application/json".to_string())]);
	for (name, value) in headers {
		response_headers.insert(name.to_string(), value.to_string());
	}
	HttpResponse::from_bytes(status, response_headers, json_stringify(&body))
}

/// A `vi.fn` fetch: answers with `respond` and records every request.
fn recording_fetch(
	respond: impl Fn(&HttpRequest) -> LocalBoxFuture<'static, PiResult<HttpResponse>> + 'static,
) -> (FetchFunction, Rc<RefCell<Vec<HttpRequest>>>) {
	let requests: Rc<RefCell<Vec<HttpRequest>>> = Rc::default();
	let recorded = requests.clone();
	let fetch: FetchFunction = Rc::new(move |request| {
		let response = respond(&request);
		recorded.borrow_mut().push(request);
		response
	});
	(fetch, requests)
}

fn ready(response: HttpResponse) -> LocalBoxFuture<'static, PiResult<HttpResponse>> {
	Box::pin(async move { Ok(response) })
}

type OnResponseCalls = Rc<RefCell<Vec<(u16, ImageModel)>>>;

/// A `vi.fn` `onResponse` recording the status and model of each call.
fn recording_on_response() -> (OnResponse<ImageModel>, OnResponseCalls) {
	let calls: OnResponseCalls = Rc::default();
	let recorded = calls.clone();
	let on_response: OnResponse<ImageModel> = Rc::new(
		move |response: ProviderResponse, model: &ImageModel| -> LocalBoxFuture<'static, PiResult<()>> {
			recorded.borrow_mut().push((response.status, model.clone()));
			Box::pin(async { Ok(()) })
		},
	);
	(on_response, calls)
}

fn options(configure: impl FnOnce(&mut ImagesOptions)) -> Option<ImagesOptions> {
	let mut options = ImagesOptions::default();
	configure(&mut options);
	Some(options)
}

fn json_body(request: &HttpRequest) -> Value {
	serde_json::from_slice(request.body.as_deref().expect("request has a body")).expect("request body is JSON")
}

/// `expect(actual).toMatchObject(expected)` for JSON values; numbers compare as `f64`, like JS numbers.
fn assert_match_object(actual: &Value, expected: &Value) {
	match (actual, expected) {
		(Value::Object(actual), Value::Object(expected)) => {
			for (key, expected) in expected {
				assert_match_object(actual.get(key).unwrap_or_else(|| panic!("missing key {key}")), expected);
			}
		}
		(Value::Array(actual), Value::Array(expected)) => {
			assert_eq!(actual.len(), expected.len());
			for (actual, expected) in actual.iter().zip(expected) {
				assert_match_object(actual, expected);
			}
		}
		(Value::Number(actual), Value::Number(expected)) => assert_eq!(actual.as_f64(), expected.as_f64()),
		_ => assert_eq!(actual, expected),
	}
}

fn assert_result_matches(result: &AssistantImages, expected: Value) {
	assert_match_object(&serde_json::to_value(result).unwrap(), &expected);
}

mod openai_codex_images {
	use super::*;

	#[test]
	fn generates_images_through_the_native_endpoint_with_base_url_suffix() {
		for suffix in ["", "/codex", "/codex/responses", "/codex/"] {
			pi_host::testing::run(|_host| async move {
				let (fetch, requests) = recording_fetch(|_| {
					ready(json_response(
						json!({ "created": 1, "data": [{ "b64_json": IMAGE_DATA }] }),
						200,
						&[("x-codex-imagegen-request-id", "image-request-1")],
					))
				});
				let (on_response, on_response_calls) = recording_on_response();
				let mut model_with_suffix = model();
				model_with_suffix.base_url = format!("{}{suffix}", model().base_url);
				let result = generate_images(
					&model_with_suffix,
					&context(),
					options(|options| {
						options.api_key = Some(mock_token());
						options.fetch = Some(fetch);
						options.on_response = Some(on_response);
						options.identity = Some(
							serde_json::from_value(json!({ "userAgent": { "custom": "image-test-client" } })).unwrap(),
						);
						options.headers = Some(IndexMap::from([(
							"x-test-header".to_string(),
							Some("preserved".to_string()),
						)]));
					}),
				)
				.await
				.unwrap();

				let case = format!("base URL suffix {suffix:?}");
				let requests = requests.borrow();
				assert_eq!(requests.len(), 1, "{case}");
				let request = &requests[0];
				assert_eq!(
					request.url,
					format!("{}/codex/images/generations", model().base_url),
					"{case}"
				);
				assert_eq!(request.method, "POST", "{case}");
				assert_eq!(
					request.header("authorization"),
					Some(format!("Bearer {}", mock_token()).as_str()),
					"{case}"
				);
				assert_eq!(request.header("chatgpt-account-id"), Some("acc_test"), "{case}");
				assert!(
					request.header("x-codex-image-turn-id").is_some_and(|id| !id.is_empty()),
					"{case}"
				);
				assert_eq!(request.header("user-agent"), Some("image-test-client"), "{case}");
				assert_eq!(request.header("x-test-header"), Some("preserved"), "{case}");
				assert!(request.signal.is_some(), "{case}");
				assert_eq!(
					json_body(request),
					json!({ "model": "gpt-image-2", "prompt": "Draw a fox", "n": 1 }),
					"{case}"
				);
				assert_result_matches(
					&result,
					json!({
						"stopReason": "stop",
						"responseId": "image-request-1",
						"output": [{ "type": "image", "data": IMAGE_DATA, "mimeType": "image/png" }],
					}),
				);
				let on_response_calls = on_response_calls.borrow();
				assert_eq!(on_response_calls.len(), 1, "{case}");
				assert_eq!(on_response_calls[0].0, 200, "{case}");
				assert_eq!(on_response_calls[0].1.id, model().id, "{case}");
			});
		}
	}

	#[test]
	fn edits_reference_images_through_the_native_json_endpoint() {
		pi_host::testing::run(|_host| async move {
			let (fetch, requests) = recording_fetch(|_| {
				ready(json_response(
					json!({ "created": 1, "data": [{ "b64_json": IMAGE_DATA }] }),
					200,
					&[],
				))
			});
			let result = generate_images(
				&model(),
				&serde_json::from_value(json!({
					"input": [
						{ "type": "text", "text": "Add a hat" },
						{ "type": "image", "data": IMAGE_DATA, "mimeType": "image/png" },
					],
				}))
				.unwrap(),
				options(|options| {
					options.api_key = Some(mock_token());
					options.fetch = Some(fetch);
				}),
			)
			.await
			.unwrap();

			let requests = requests.borrow();
			assert_eq!(requests.len(), 1);
			assert_eq!(requests[0].url, format!("{}/codex/images/edits", model().base_url));
			assert_eq!(
				json_body(&requests[0]),
				json!({
					"model": "gpt-image-2",
					"prompt": "Add a hat",
					"n": 1,
					"images": [{ "image_url": format!("data:image/png;base64,{IMAGE_DATA}") }],
				})
			);
			assert_result_matches(
				&result,
				json!({
					"stopReason": "stop",
					"output": [{ "type": "image", "data": IMAGE_DATA, "mimeType": "image/png" }],
				}),
			);
		});
	}

	#[test]
	fn preserves_provider_errors_without_retrying_http() {
		for status in [400, 403, 429, 503] {
			pi_host::testing::run(|_host| async move {
				let (fetch, requests) = recording_fetch(move |_| {
					ready(json_response(
						json!({ "error": { "message": "Policy violation: cannot generate this image." } }),
						status,
						&[("x-codex-imagegen-request-id", "failed-image-request")],
					))
				});
				let (on_response, on_response_calls) = recording_on_response();
				let result = generate_images(
					&model(),
					&context(),
					options(|options| {
						options.api_key = Some(mock_token());
						options.fetch = Some(fetch);
						options.on_response = Some(on_response);
						options.max_retries = Some(9.0);
					}),
				)
				.await
				.unwrap();

				let case = format!("HTTP {status}");
				assert_result_matches(
					&result,
					json!({ "stopReason": "error", "responseId": "failed-image-request", "output": [] }),
				);
				assert!(
					result
						.error_message
						.as_deref()
						.is_some_and(|message| message.contains("Policy violation: cannot generate this image.")),
					"{case}: {:?}",
					result.error_message
				);
				assert_eq!(requests.borrow().len(), 1, "{case}");
				assert_eq!(*on_response_calls.borrow(), [(status, model())], "{case}");
			});
		}
	}

	#[test]
	fn rejects_missing_malformed_or_oversized_image_output() {
		let cases = [
			(json!([]), "ChatGPT image generation completed without an image"),
			(
				json!([{ "b64_json": "not base64" }]),
				"ChatGPT returned malformed image data",
			),
			(
				json!([{ "b64_json": IMAGE_DATA }, { "b64_json": null }]),
				"ChatGPT returned malformed image data",
			),
			(
				json!([{ "b64_json": "a".repeat(24 * 1024 * 1024 + 4) }]),
				"ChatGPT image generation output exceeds the 24 MiB base64 limit",
			),
		];
		for (index, (data, error)) in cases.into_iter().enumerate() {
			pi_host::testing::run(|_host| async move {
				let body = json_stringify(&json!({ "created": 1, "data": data }));
				let fetch: FetchFunction = Rc::new(move |_| {
					let body = body.clone();
					Box::pin(async move {
						Ok(HttpResponse::from_bytes(
							200,
							IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
							body,
						))
					})
				});
				let result = generate_images(
					&model(),
					&context(),
					options(|options| {
						options.api_key = Some(mock_token());
						options.fetch = Some(fetch);
					}),
				)
				.await
				.unwrap();

				assert_eq!(result.error_message.as_deref(), Some(error), "case {index}");
				assert_result_matches(
					&result,
					json!({ "stopReason": "error", "errorMessage": error, "output": [] }),
				);
			});
		}
	}

	#[test]
	fn honors_in_flight_cancellation() {
		pi_host::testing::run(|_host| async move {
			let controller = AbortController::new();
			let aborter = controller.clone();
			let (fetch, requests) = recording_fetch(move |request| {
				let signal = request.signal.clone();
				let aborter = aborter.clone();
				Box::pin(async move {
					let Some(signal) = signal else {
						return Err(PiError::new("Expected a request signal"));
					};
					pi_host::spawn_local(async move { aborter.abort() });
					Err(signal.aborted_future().await)
				})
			});
			let result = generate_images(
				&model(),
				&context(),
				options(|options| {
					options.api_key = Some(mock_token());
					options.fetch = Some(fetch);
					options.signal = Some(controller.signal());
				}),
			)
			.await
			.unwrap();

			assert_result_matches(&result, json!({ "stopReason": "aborted", "output": [] }));
			assert_eq!(requests.borrow().len(), 1);
		});
	}

	#[test]
	fn does_not_send_a_cancelled_request() {
		pi_host::testing::run(|_host| async move {
			let calls = Rc::new(Cell::new(0));
			let counted = calls.clone();
			let fetch: FetchFunction = Rc::new(move |_| {
				counted.set(counted.get() + 1);
				Box::pin(async { Err(PiError::new("vi.fn() returns undefined")) })
			});
			let controller = AbortController::new();
			controller.abort();
			let result = generate_images(
				&model(),
				&context(),
				options(|options| {
					options.api_key = Some(mock_token());
					options.fetch = Some(fetch);
					options.signal = Some(controller.signal());
				}),
			)
			.await
			.unwrap();

			assert_eq!(result.stop_reason, pi_ai::types::ImagesStopReason::Aborted);
			assert_eq!(calls.get(), 0);
		});
	}

	/// A JSON response whose body never sends a chunk and fails with `error` once it resolves.
	fn failing_body_response(error: LocalBoxFuture<'static, PiError>) -> HttpResponse {
		let mut response = json_response(Value::Null, 200, &[]);
		response.body = futures::stream::once(async move { Err::<Bytes, PiError>(error.await) }).boxed_local();
		response
	}

	#[test]
	fn bounds_the_response_body_with_the_image_timeout() {
		pi_host::testing::run(|_host| async move {
			let (fetch, requests) = recording_fetch(|request| {
				let signal = request.signal.clone();
				Box::pin(async move {
					Ok(failing_body_response(Box::pin(async move {
						match signal {
							Some(signal) => signal.aborted_future().await,
							None => futures::future::pending().await,
						}
					})))
				})
			});
			let result = generate_images(
				&model(),
				&context(),
				options(|options| {
					options.api_key = Some(mock_token());
					options.fetch = Some(fetch);
					options.timeout_ms = Some(25.0);
				}),
			)
			.await
			.unwrap();

			assert_result_matches(
				&result,
				json!({
					"stopReason": "error",
					"errorMessage": "ChatGPT image generation timed out after 25ms",
					"output": [],
				}),
			);
			assert_eq!(requests.borrow().len(), 1);
		});
	}

	#[test]
	fn normalizes_an_sdk_timeout_before_the_image_timeout_signal_fires() {
		pi_host::testing::run(|_host| async move {
			let started_at = pi_host::monotonic_ms();
			let (fetch, requests) = recording_fetch(|_| {
				Box::pin(async {
					Ok(failing_body_response(Box::pin(async {
						PiError::new("Request timed out.").with_data(json!({ "headers": null }))
					})))
				})
			});
			let result = generate_images(
				&model(),
				&context(),
				options(|options| {
					options.api_key = Some(mock_token());
					options.fetch = Some(fetch);
					options.timeout_ms = Some(60_000.0);
				}),
			)
			.await
			.unwrap();

			assert_result_matches(
				&result,
				json!({
					"stopReason": "error",
					"errorMessage": "ChatGPT image generation timed out after 60000ms",
					"output": [],
				}),
			);
			assert!(pi_host::monotonic_ms() - started_at < 60_000.0);
			assert_eq!(requests.borrow().len(), 1);
		});
	}

	#[test]
	fn passes_the_native_payload_to_hooks_and_rejects_invalid_replacements() {
		pi_host::testing::run(|_host| async move {
			let (fetch, requests) = recording_fetch(|_| {
				ready(json_response(
					json!({ "created": 1, "data": [{ "b64_json": IMAGE_DATA }] }),
					200,
					&[],
				))
			});
			let hook_calls: Rc<RefCell<Vec<(Value, ImageModel)>>> = Rc::default();
			let recorded = hook_calls.clone();
			let on_payload: OnPayload<ImageModel> = Rc::new(
				move |payload: Value, model: &ImageModel| -> LocalBoxFuture<'static, PiResult<Option<Value>>> {
					recorded.borrow_mut().push((payload.clone(), model.clone()));
					let mut next = payload.as_object().cloned().unwrap_or_default();
					next.insert("quality".into(), json!("low"));
					Box::pin(async move { Ok(Some(Value::Object(next))) })
				},
			);
			let result = generate_images(
				&model(),
				&context(),
				options(|options| {
					options.api_key = Some(mock_token());
					options.fetch = Some(fetch.clone());
					options.on_payload = Some(on_payload);
				}),
			)
			.await
			.unwrap();
			{
				let hook_calls = hook_calls.borrow();
				assert_eq!(hook_calls.len(), 1);
				assert_eq!(hook_calls[0].1, model());
				assert_eq!(
					hook_calls[0].0,
					json!({ "model": "gpt-image-2", "prompt": "Draw a fox", "n": 1 })
				);
				let requests = requests.borrow();
				assert_eq!(requests.len(), 1);
				assert_eq!(
					json_body(&requests[0]),
					json!({ "model": "gpt-image-2", "prompt": "Draw a fox", "n": 1, "quality": "low" })
				);
			}
			assert_eq!(result.stop_reason, pi_ai::types::ImagesStopReason::Stop);
			requests.borrow_mut().clear();
			let null_payload: OnPayload<ImageModel> = Rc::new(
				|_payload: Value, _model: &ImageModel| -> LocalBoxFuture<'static, PiResult<Option<Value>>> {
					Box::pin(async { Ok(Some(Value::Null)) })
				},
			);
			let invalid = generate_images(
				&model(),
				&context(),
				options(|options| {
					options.api_key = Some(mock_token());
					options.fetch = Some(fetch);
					options.on_payload = Some(null_payload);
				}),
			)
			.await
			.unwrap();
			assert_result_matches(
				&invalid,
				json!({
					"stopReason": "error",
					"errorMessage": "Codex image generation payload hook must return an object",
				}),
			);
			assert!(requests.borrow().is_empty());
		});
	}

	#[test]
	fn registers_chatgpt_before_openrouter_in_the_built_in_image_inventory() {
		assert_eq!(
			builtin_images_providers()
				.iter()
				.map(|provider| provider.id.as_str())
				.collect::<Vec<_>>(),
			["openai-codex", "openrouter"]
		);
	}

	#[test]
	#[allow(deprecated)]
	fn generates_codex_images_through_the_unified_collection_image_only_factory_and_global_compat_api() {
		pi_host::testing::run(|_host| async move {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let access = mock_token();
			credentials
				.modify(
					"openai-codex",
					Box::new(move |_| {
						Box::pin(async move {
							Ok(Some(Credential::OAuth(OAuthCredential::new(
								"mock-refresh-token",
								access,
								pi_host::now_ms() + 3_600_000.0,
							))))
						})
					}),
					None,
				)
				.await
				.unwrap();
			let models = builtin_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone()),
				..CreateModelsOptions::default()
			}));
			let Some(AnyModel::Image(model_of_type)) =
				models.get_model_of_type(ModelType::Image, "openai-codex", "chatgpt-image-generation")
			else {
				panic!("the image model is registered");
			};
			assert_match_object(
				&serde_json::to_value(&model_of_type).unwrap(),
				&serde_json::to_value(model()).unwrap(),
			);
			assert_eq!(
				get_builtin_image_model("openai-codex", "chatgpt-image-generation"),
				Some(model_of_type.clone())
			);
			assert_eq!(
				pi_ai::compat::get_image_model("openai-codex", "chatgpt-image-generation"),
				Some(model_of_type.clone())
			);
			let image_models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone()),
				..CreateModelsOptions::default()
			}));
			image_models.set_provider(Rc::new(openai_codex_images_provider()));
			assert!(image_models.get_models(None).is_empty());
			assert_eq!(
				image_models.get_models_of_type(ModelType::Image, None),
				[AnyModel::Image(model_of_type.clone())]
			);
			let fetch: FetchFunction = Rc::new(|_| {
				ready(json_response(
					json!({ "created": 1, "data": [{ "b64_json": IMAGE_DATA }] }),
					200,
					&[],
				))
			});
			let models_options = || {
				let mut options = ModelsImagesOptions::default();
				options.base.base.fetch = Some(fetch.clone());
				Some(options)
			};
			let mut compat_options = ProviderImagesOptions::default();
			compat_options.base.base.fetch = Some(fetch.clone());
			compat_options.base.base.api_key = Some(mock_token());
			for result in [
				models
					.generate_images(&model_of_type, &context(), models_options())
					.await,
				image_models
					.generate_images(&model_of_type, &context(), models_options())
					.await,
				pi_ai::compat::generate_images(&model_of_type, &context(), Some(compat_options))
					.await
					.unwrap(),
			] {
				assert_result_matches(
					&result,
					json!({
						"stopReason": "stop",
						"output": [{ "type": "image", "data": IMAGE_DATA, "mimeType": "image/png" }],
					}),
				);
			}
		});
	}
}
