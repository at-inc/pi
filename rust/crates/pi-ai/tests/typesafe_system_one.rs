//! Port of `packages/ai/test/typesafe-system-one.test.ts`.
//!
//! `vi.fn` fetch mocks are `FetchFunction`s that record each request. Header checks read the request's header map
//! case-insensitively, like `new Headers(init.headers)`.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::typesafe_system_one::classify;
use pi_ai::types::{
	BaseModel, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierStopReason, ModelCost,
	ModelInputModality, ModelType, ProviderHeaders, ProviderRequestOptions,
};
use pi_host::testing::run;
use pi_host::{AbortSignal, FetchFunction, HttpRequest, HttpResponse};
use serde_json::{Value, json};

fn model() -> ClassifierModel {
	ClassifierModel {
		base: BaseModel {
			id: "jev-latest".into(),
			name: "Jev".into(),
			api: "typesafe-system-one".into(),
			provider: "typesafe".into(),
			base_url: "https://api.typesafe.ai/v1/".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		r#type: ModelType::Classifier,
		context_window: 64000.0,
		compat: None,
		extra: Default::default(),
	}
}

fn image_model() -> ClassifierModel {
	let mut model = model();
	model.input = vec![ModelInputModality::Text, ModelInputModality::Image];
	model
}

fn context_json() -> Value {
	json!({
		"state": { "text": "The deployment succeeded, thank you." },
		"questions": {
			"category": {
				"type": "choice",
				"instructions": "Classify the message",
				"criteria": { "success": "Successful", "failure": "Failed" },
			},
			"satisfaction": {
				"type": "score",
				"instructions": "Score satisfaction",
				"criteria": ["low", "neutral", "high"],
			},
			"approved": {
				"type": "bool",
				"instructions": "Does the user approve?",
				"criteria": { "true": "Approval", "false": "No approval" },
			},
		},
	})
}

fn context() -> ClassifierContext {
	serde_json::from_value(context_json()).unwrap()
}

fn context_with_images(images: Value) -> ClassifierContext {
	let mut value = context_json();
	value["images"] = images;
	serde_json::from_value(value).unwrap()
}

fn png() -> Value {
	json!([{ "type": "image", "data": "cG5n", "mimeType": "image/png" }])
}

fn wire_answers() -> Value {
	json!({
		"category": {
			"type": "choice",
			"choice": "success",
			"probabilities": { "success": 0.9, "failure": 0.1 },
			"confidence": 0.8,
		},
		"satisfaction": { "type": "score", "score": 2, "confidence": 0.7 },
		"approved": { "type": "noul", "noul": 0.95 },
	})
}

/// `Response.json(body)`.
fn json_response(status: u16, headers: &[(&str, &str)], body: &Value) -> HttpResponse {
	let mut map: IndexMap<String, String> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
	map.insert("content-type".into(), "application/json".into());
	HttpResponse::from_bytes(status, map, pi_host::js::json_stringify(body))
}

type Recorded = Rc<RefCell<Vec<HttpRequest>>>;

/// A `vi.fn` fetch that records its requests and answers each with `respond`.
fn recording_fetch(respond: impl Fn(&HttpRequest) -> HttpResponse + 'static) -> (FetchFunction, Recorded) {
	let requests: Recorded = Rc::new(RefCell::new(Vec::new()));
	let recorded = requests.clone();
	let fetch: FetchFunction = Rc::new(move |request: HttpRequest| {
		let response = respond(&request);
		recorded.borrow_mut().push(request);
		Box::pin(async move { Ok(response) })
	});
	(fetch, requests)
}

fn answers_fetch() -> (FetchFunction, Recorded) {
	recording_fetch(|_| json_response(200, &[], &json!({ "answers": wire_answers() })))
}

fn body(request: &HttpRequest) -> Value {
	serde_json::from_slice(request.body.as_deref().unwrap()).unwrap()
}

fn options(fetch: FetchFunction) -> ClassifierOptions {
	ClassifierOptions {
		base: ProviderRequestOptions {
			api_key: Some("secret".into()),
			fetch: Some(fetch),
			..ProviderRequestOptions::default()
		},
		temperature: None,
	}
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

fn assert_close(actual: f64, expected: f64, digits: i32) {
	assert!(
		(actual - expected).abs() < 10f64.powi(-digits) / 2.0,
		"expected {actual} to be close to {expected}"
	);
}

mod type_safe_system_one {
	use super::*;

	#[test]
	fn maps_public_bool_questions_and_answers_to_typesafe_noul_values() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let mut first = options(fetch);
			first.temperature = Some(1.5);
			let result = classify(&model(), &context(), Some(first)).await.unwrap();
			let mut priced = model();
			priced.cost.input = 0.042;
			let (priced_fetch, _) = recording_fetch(|_| {
				json_response(
					200,
					&[],
					&json!({ "answers": wire_answers(), "usage": { "input_tokens": 308, "output_tokens": 23 } }),
				)
			});
			let priced_result = classify(&priced, &context(), Some(options(priced_fetch)))
				.await
				.unwrap();

			let requests = requests.borrow();
			assert_eq!(requests.len(), 1);
			let payload = body(&requests[0]);
			assert_eq!(payload["model"], "jev-latest");
			assert_eq!(payload["questions"]["category"]["type"], "choice");
			assert_eq!(payload["questions"]["satisfaction"]["type"], "score");
			assert_eq!(payload["questions"]["approved"]["type"], "noul");
			// System One has no temperature field; the option is ignored.
			assert!(payload.get("temperature").is_none());
			assert_eq!(requests[0].header("authorization"), Some("Bearer secret"));
			assert_eq!(requests[0].url, "https://api.typesafe.ai/v1/systemone");
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(
				to_json(&result.answers["approved"]),
				json!({ "type": "bool", "probability": 0.95 })
			);
			let category = to_json(&result.answers["category"]);
			assert_eq!(category["type"], "choice");
			assert_eq!(category["choice"], "success");
			assert_eq!(
				to_json(&result.answers["satisfaction"]),
				json!({ "type": "score", "score": 2.0, "confidence": 0.7 })
			);
			assert!(result.usage.is_none());
			let usage = priced_result.usage.unwrap();
			assert_eq!((usage.input, usage.output, usage.total_tokens), (308.0, 23.0, 331.0));
			assert_close(usage.cost.total, 0.000012936, 12);
		});
	}

	#[test]
	fn posts_openrouter_system_one_requests_to_its_typesafe_compatible_endpoint() {
		run(|_| async {
			let (fetch, requests) = recording_fetch(|_| {
				// Response shape observed from the live OpenRouter endpoint.
				json_response(
					200,
					&[],
					&json!({
						"id": "gen-dec-1",
						"provider": "TypeSafe",
						"answers": wire_answers(),
						"usage": { "input_tokens": 308, "output_tokens": 23, "cost": 0.000012936 },
					}),
				)
			});
			let mut open_router = model();
			open_router.id = "typesafe/jev-1.13".into();
			open_router.provider = "openrouter".into();
			open_router.base_url = "https://openrouter.ai/api/v1".into();
			open_router.cost.input = 0.042;

			let result = classify(&open_router, &context(), Some(options(fetch))).await.unwrap();

			let requests = requests.borrow();
			let payload = body(&requests[0]);
			assert_eq!(payload["model"], "typesafe/jev-1.13");
			assert_eq!(payload["state"], context_json()["state"]);
			assert_eq!(requests[0].url, "https://openrouter.ai/api/v1/systemone");
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(
				to_json(&result.answers["approved"]),
				json!({ "type": "bool", "probability": 0.95 })
			);
			// Priced from the catalog like chat usage; matches OpenRouter's reported cost.
			assert_close(result.usage.unwrap().cost.total, 0.000012936, 12);
		});
	}

	#[test]
	fn rejects_models_for_other_classifier_apis() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let mut other = model();
			other.api = "cloudflare-workers-ai-system-one".into();
			let result = classify(&other, &context(), Some(options(fetch))).await.unwrap();

			assert!(requests.borrow().is_empty());
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert!(
				result
					.error_message
					.unwrap()
					.contains("Unsupported classifier API: cloudflare-workers-ai-system-one")
			);
		});
	}

	#[test]
	fn rejects_image_input_for_text_only_models_before_sending() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let images = json!([{ "type": "image", "data": "aW1hZ2U=", "mimeType": "image/png" }]);
			let result = classify(&model(), &context_with_images(images), Some(options(fetch)))
				.await
				.unwrap();

			assert!(requests.borrow().is_empty());
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("System One API does not support image input for jev-latest")
			);
		});
	}

	#[test]
	fn sends_images_to_typesafe_as_assets_with_the_opt_in_header() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let images = json!([
				{ "type": "image", "data": "cG5n", "mimeType": "image/png" },
				{ "type": "image", "data": "anBlZw==", "mimeType": "image/jpeg" },
			]);
			let result = classify(
				&image_model(),
				&context_with_images(images),
				Some(options(fetch.clone())),
			)
			.await
			.unwrap();
			let without_images = classify(&image_model(), &context(), Some(options(fetch)))
				.await
				.unwrap();

			let requests = requests.borrow();
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			let first = body(&requests[0]);
			assert_eq!(
				first["state"],
				json!(["<image-1>", "<image-2>", context_json()["state"]])
			);
			assert_eq!(
				first["assets"],
				json!({
					"<image-1>": { "type": "image", "format": "png", "data_base64": "cG5n" },
					"<image-2>": { "type": "image", "format": "jpeg", "data_base64": "anBlZw==" },
				})
			);
			assert_eq!(requests[0].header("typesafe-danger-zone-images"), Some("true"));
			// Requests without images keep the plain state and send no opt-in header.
			assert_eq!(without_images.stop_reason, ClassifierStopReason::Stop);
			let second = body(&requests[1]);
			assert_eq!(second["state"], context_json()["state"]);
			assert!(second.get("assets").is_none());
			assert!(requests[1].header("typesafe-danger-zone-images").is_none());
		});
	}

	#[test]
	fn rejects_image_formats_typesafe_does_not_accept_before_sending() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let images = json!([{ "type": "image", "data": "d2VicA==", "mimeType": "image/webp" }]);
			let result = classify(&image_model(), &context_with_images(images), Some(options(fetch)))
				.await
				.unwrap();

			assert!(requests.borrow().is_empty());
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("System One API accepts PNG and JPEG images for jev-latest, got image/webp")
			);
		});
	}

	#[test]
	fn sends_images_to_openrouter_as_image_url_parts_in_the_state() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let mut open_router = image_model();
			open_router.id = "typesafe/jev-1.14".into();
			open_router.provider = "openrouter".into();
			open_router.base_url = "https://openrouter.ai/api/v1".into();

			let result = classify(&open_router, &context_with_images(png()), Some(options(fetch)))
				.await
				.unwrap();

			let requests = requests.borrow();
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			// OpenRouter answers 200 for `assets` but drops them, and rejects a top-level `images` list.
			let payload = body(&requests[0]);
			let keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
			assert_eq!(keys, ["model", "state", "questions"]);
			assert_eq!(payload["model"], "typesafe/jev-1.14");
			assert_eq!(
				payload["state"],
				json!([{ "type": "image_url", "image_url": { "url": "data:image/png;base64,cG5n" } }, context_json()["state"]])
			);
			assert!(payload["questions"].is_object());
			assert!(requests[0].header("typesafe-danger-zone-images").is_none());
		});
	}

	fn sends_openrouters_image_form_to(provider: &str, base_url: &str, compat: Option<Value>) {
		let (provider, base_url) = (provider.to_string(), base_url.to_string());
		run(|_| async move {
			let (fetch, requests) = answers_fetch();
			let mut target = image_model();
			target.id = "typesafe/jev-1.14".into();
			target.provider = provider.as_str().into();
			target.base_url = base_url;
			target.compat = compat.map(|compat| compat.as_object().unwrap().clone());

			let result = classify(&target, &context_with_images(png()), Some(options(fetch)))
				.await
				.unwrap();

			let requests = requests.borrow();
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			let payload = body(&requests[0]);
			assert_eq!(
				payload["state"],
				json!([{ "type": "image_url", "image_url": { "url": "data:image/png;base64,cG5n" } }, context_json()["state"]])
			);
			assert!(payload.get("assets").is_none());
			assert!(requests[0].header("typesafe-danger-zone-images").is_none());
		});
	}

	#[test]
	fn sends_openrouters_image_form_to_openrouter_behind_a_proxy() {
		sends_openrouters_image_form_to("openrouter", "https://proxy.example.com/v1", None);
	}

	#[test]
	fn sends_openrouters_image_form_to_a_regional_openrouter_endpoint() {
		sends_openrouters_image_form_to("custom", "https://us.openrouter.ai/api/v1", None);
	}

	#[test]
	fn sends_openrouters_image_form_to_a_proxy_declared_through_compat() {
		sends_openrouters_image_form_to(
			"custom",
			"https://proxy.example.com/v1",
			Some(json!({ "imageInput": "state-image-urls", "sendImageOptInHeader": false })),
		);
	}

	#[test]
	fn lets_compat_override_detected_image_settings_per_field() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let mut target = image_model();
			target.compat = Some(json!({ "sendImageOptInHeader": false }).as_object().unwrap().clone());

			// Only the header is overridden; the detected `assets` form stays.
			classify(&target, &context_with_images(png()), Some(options(fetch)))
				.await
				.unwrap();

			let requests = requests.borrow();
			assert!(body(&requests[0]).get("assets").is_some());
			assert!(requests[0].header("typesafe-danger-zone-images").is_none());
		});
	}

	#[test]
	fn merges_headers_case_insensitively_and_supports_null_suppression() {
		run(|_| async {
			let (fetch, requests) = answers_fetch();
			let mut with_headers = model();
			with_headers.headers = Some(IndexMap::from([
				("authorization".to_string(), "Bearer model".to_string()),
				("X-Source".to_string(), "model".to_string()),
			]));

			let mut first = options(fetch.clone());
			first.headers = Some(ProviderHeaders::from([
				("Authorization".to_string(), Some("Bearer request".to_string())),
				("x-source".to_string(), Some("request".to_string())),
			]));
			classify(&with_headers, &context(), Some(first)).await.unwrap();
			let mut second = options(fetch);
			second.headers = Some(ProviderHeaders::from([("Authorization".to_string(), None)]));
			classify(&with_headers, &context(), Some(second)).await.unwrap();

			let requests = requests.borrow();
			assert_eq!(requests[0].header("authorization"), Some("Bearer request"));
			assert_eq!(requests[0].header("x-source"), Some("request"));
			assert_eq!(
				requests[0]
					.headers
					.keys()
					.filter(|name| name.eq_ignore_ascii_case("authorization"))
					.count(),
				1
			);
			assert!(requests[1].header("authorization").is_none());
		});
	}

	#[test]
	fn preserves_prototype_sensitive_question_ids_in_answers() {
		run(|_| async {
			let prototype_context: ClassifierContext = serde_json::from_value(json!({
				"state": {},
				"questions": {
					"__proto__": {
						"type": "bool",
						"instructions": "Is this true?",
						"criteria": { "true": "Yes", "false": "No" },
					},
				},
			}))
			.unwrap();
			let (fetch, _) = recording_fetch(|_| {
				json_response(
					200,
					&[],
					&json!({ "answers": { "__proto__": { "type": "noul", "noul": 0.75 } } }),
				)
			});
			let result = classify(&model(), &prototype_context, Some(options(fetch)))
				.await
				.unwrap();

			let serialized = to_json(&result.answers);
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(result.answers.keys().collect::<Vec<_>>(), ["__proto__"]);
			assert_eq!(
				to_json(&result.answers["__proto__"]),
				json!({ "type": "bool", "probability": 0.75 })
			);
			assert_eq!(serialized["__proto__"], json!({ "type": "bool", "probability": 0.75 }));
		});
	}

	#[test]
	fn reports_request_timeouts_separately_from_caller_cancellation() {
		run(|_| async {
			let fetch: FetchFunction = Rc::new(|request: HttpRequest| {
				Box::pin(async move {
					let signal = request.signal.expect("missing request signal");
					Err(signal.aborted_future().await)
				})
			});
			let mut timed = options(fetch);
			timed.timeout_ms = Some(5.0);
			timed.max_retries = Some(0.0);
			let result = classify(&model(), &context(), Some(timed)).await.unwrap();

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert_eq!(result.error_message.as_deref(), Some("Request timed out after 5ms"));
		});
	}

	#[test]
	fn creates_a_fresh_timeout_for_every_retry_attempt() {
		run(|_| async {
			let signals: Rc<RefCell<Vec<AbortSignal>>> = Rc::new(RefCell::new(Vec::new()));
			let seen = signals.clone();
			let fetch: FetchFunction = Rc::new(move |request: HttpRequest| {
				let signal = request.signal.expect("missing request signal");
				seen.borrow_mut().push(signal);
				let attempt = seen.borrow().len();
				Box::pin(async move {
					Ok(if attempt == 1 {
						HttpResponse::from_bytes(
							500,
							IndexMap::from([("retry-after-ms".to_string(), "0".to_string())]),
							"retry",
						)
					} else {
						json_response(200, &[], &json!({ "answers": wire_answers() }))
					})
				})
			});
			let mut retried = options(fetch);
			retried.timeout_ms = Some(1000.0);
			retried.max_retries = Some(1.0);
			let result = classify(&model(), &context(), Some(retried)).await.unwrap();

			let signals = signals.borrow();
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(signals.len(), 2);
			assert!(!AbortSignal::ptr_eq(&signals[0], &signals[1]));
		});
	}

	#[test]
	fn returns_malformed_responses_as_classifier_errors() {
		run(|_| async {
			let (fetch, _) = recording_fetch(|_| {
				json_response(
					200,
					&[],
					&json!({ "answers": {}, "usage": { "input_tokens": 10, "output_tokens": 2 } }),
				)
			});
			let result = classify(&model(), &context(), Some(options(fetch))).await.unwrap();

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert!(result.answers.is_empty());
			assert!(
				result
					.error_message
					.unwrap()
					.contains("did not return an answer for category")
			);
			// The request was billed, so its usage is kept.
			let usage = result.usage.unwrap();
			assert_eq!((usage.input, usage.output), (10.0, 2.0));
		});
	}

	#[test]
	fn ignores_malformed_usage() {
		run(|_| async {
			let (fetch, _) = recording_fetch(|_| {
				json_response(
					200,
					&[],
					&json!({ "answers": wire_answers(), "usage": { "input_tokens": "many", "output_tokens": 3 } }),
				)
			});
			let result = classify(&model(), &context(), Some(options(fetch))).await.unwrap();
			let (fetch, _) = recording_fetch(|_| {
				json_response(
					200,
					&[],
					&json!({ "answers": wire_answers(), "usage": { "cost": 0.1 } }),
				)
			});
			let without_tokens = classify(&model(), &context(), Some(options(fetch))).await.unwrap();

			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			let usage = result.usage.unwrap();
			assert_eq!((usage.input, usage.output, usage.total_tokens), (0.0, 3.0, 3.0));
			assert_eq!(without_tokens.stop_reason, ClassifierStopReason::Stop);
			assert!(without_tokens.usage.is_none());
		});
	}
}
