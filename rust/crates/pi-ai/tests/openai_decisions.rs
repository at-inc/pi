//! Port of `packages/ai/test/openai-decisions.test.ts`.
//!
//! Every case posts through `api::classifier_shared::post_classifier_request`, a `PORT-STUB: w3-bedrock-misc` on this
//! branch, so the suite is `#[ignore]`d until that file lands. It was run against a local stand-in of
//! `classifier-shared.ts` (not committed) and passed.

mod openai_responses_support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use openai_responses_support::{assert_match_object, json_response, response};
use pi_ai::api::openai_decisions::classify;
use pi_ai::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierStopReason};
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpRequest, HttpResponse};
use serde_json::{Value, json};

fn model() -> ClassifierModel {
	serde_json::from_value(json!({
		"type": "classifier",
		"id": "gpt-6-luna",
		"name": "GPT-6 Luna",
		"api": "openai-decisions",
		"provider": "openai",
		"baseUrl": "https://api.openai.com/v1",
		"input": ["text", "image"],
		"cost": {
			"input": 0.1,
			"output": 0,
			"cacheRead": 0,
			"cacheWrite": 0,
			"tiers": [{ "inputTokensAbove": 272000, "input": 0.2, "output": 0, "cacheRead": 0, "cacheWrite": 0 }],
		},
		"contextWindow": 922000,
	}))
	.expect("classifier model JSON")
}

fn context_json() -> Value {
	json!({
		"state": { "text": "The deployment succeeded, thank you." },
		"questions": {
			"category": {
				"type": "choice",
				"instructions": "Classify the message",
				"criteria": { "success": "Successful", "failure": "" },
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
	serde_json::from_value(context_json()).expect("classifier context JSON")
}

/// Response shape from the API reference and live `gpt-6-luna` requests.
fn wire_answers() -> Vec<Value> {
	vec![
		json!({
			"type": "choice",
			"name": "category",
			"choice": "success",
			"probabilities": [{ "value": "success", "probability": 0.9 }, { "value": "failure", "probability": 0.1 }],
			"confidence": 0.8,
		}),
		json!({
			"type": "score",
			"name": "satisfaction",
			"score": 1.8,
			"probabilities": [
				{ "value": 0, "label": "low", "probability": 0.05 },
				{ "value": 1, "label": "neutral", "probability": 0.1 },
				{ "value": 2, "label": "high", "probability": 0.85 },
			],
			"confidence": 0.7,
		}),
		json!({ "type": "predicate", "name": "approved", "probability": 0.95 }),
	]
}

fn wire_usage() -> Value {
	json!({
		"input_tokens": 164,
		"input_tokens_details": { "cached_tokens": 0, "cache_write_tokens": 0 },
		"output_tokens": 0,
		"output_tokens_details": { "reasoning_tokens": 0 },
		"total_tokens": 164,
	})
}

fn image() -> Value {
	json!({ "type": "image", "data": "aW1hZ2U=", "mimeType": "image/png" })
}

/// A `vi.fn` fetch: records every request and answers with `make`.
fn recording_fetch(make: impl Fn(usize) -> HttpResponse + 'static) -> (FetchFunction, Rc<RefCell<Vec<HttpRequest>>>) {
	let calls = Rc::new(RefCell::new(Vec::new()));
	let recorded = calls.clone();
	let fetch: FetchFunction = Rc::new(move |request| {
		recorded.borrow_mut().push(request);
		let response = make(recorded.borrow().len());
		Box::pin(async move { Ok(response) })
	});
	(fetch, calls)
}

fn options(configure: impl FnOnce(&mut ClassifierOptions)) -> ClassifierOptions {
	let mut options = ClassifierOptions::default();
	configure(&mut options);
	options
}

fn with_key(fetch: FetchFunction) -> ClassifierOptions {
	options(|options| {
		options.api_key = Some("secret".into());
		options.fetch = Some(fetch);
	})
}

fn answers_json(result: &pi_ai::types::ClassifierResult) -> Value {
	serde_json::to_value(&result.answers).expect("answers serialize")
}

fn close(actual: f64, expected: f64) {
	assert!((actual - expected).abs() < 5e-13, "{actual} vs {expected}");
}

mod open_ai_decisions {
	use super::*;

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn maps_questions_to_decisions_types_and_answers_back_by_name() {
		run(|_| async {
			let (fetch, calls) = recording_fetch(|_| {
				// Answers out of question order: they are matched by name.
				let mut answers = wire_answers();
				answers.reverse();
				json_response(
					200,
					&json!({ "model": "gpt-6-luna", "answers": answers, "usage": wire_usage() }),
				)
			});
			let mut options = with_key(fetch);
			options.temperature = Some(1.5);

			let result = classify(&model(), &context(), Some(&options)).await;

			let calls = calls.borrow();
			assert_eq!(calls.len(), 1);
			assert_eq!(calls[0].url, "https://api.openai.com/v1/decisions");
			assert_eq!(calls[0].header("authorization"), Some("Bearer secret"));
			let body: Value = serde_json::from_slice(calls[0].body.as_deref().unwrap()).unwrap();
			assert_eq!(
				body,
				json!({
					"model": "gpt-6-luna",
					"input": pi_host::js::json_stringify(&context_json()["state"]),
					"questions": [
						{
							"type": "choice",
							"name": "category",
							"instructions": "Classify the message",
							// Empty descriptions are omitted.
							"choices": [{ "value": "success", "description": "Successful" }, { "value": "failure" }],
						},
						{
							"type": "score",
							"name": "satisfaction",
							"instructions": "Score satisfaction",
							"levels": [{ "label": "low" }, { "label": "neutral" }, { "label": "high" }],
						},
						{
							"type": "predicate",
							"name": "approved",
							"instructions": "Does the user approve?\n\nTrue means: Approval\nFalse means: No approval",
						},
					],
				})
			);
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(
				answers_json(&result),
				json!({
					"category": {
						"type": "choice",
						"choice": "success",
						"probabilities": { "success": 0.9, "failure": 0.1 },
						"confidence": 0.8,
					},
					"satisfaction": { "type": "score", "score": 1.8, "confidence": 0.7 },
					"approved": { "type": "bool", "probability": 0.95 },
				})
			);
			let usage = result.usage.expect("usage");
			assert_match_object(
				&serde_json::to_value(&usage).unwrap(),
				&json!({ "input": 164, "output": 0, "cacheRead": 0, "totalTokens": 164 }),
			);
			close(usage.cost.total, 0.0000164);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn prices_long_context_requests_at_the_long_context_input_rate() {
		run(|_| async {
			let (fetch, _) = recording_fetch(|_| {
				json_response(
					200,
					&json!({ "answers": wire_answers(), "usage": { "input_tokens": 300000, "output_tokens": 0 } }),
				)
			});

			let result = classify(&model(), &context(), Some(&with_key(fetch))).await;

			close(result.usage.expect("usage").cost.total, 0.06);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn sends_images_after_the_state_in_one_user_message() {
		run(|_| async {
			let (fetch, calls) = recording_fetch(|_| json_response(200, &json!({ "answers": wire_answers() })));
			let mut jpeg = image();
			jpeg["mimeType"] = json!("image/jpeg");
			let mut context_value = context_json();
			context_value["images"] = json!([image(), jpeg]);
			let context: ClassifierContext = serde_json::from_value(context_value).unwrap();

			let result = classify(&model(), &context, Some(&with_key(fetch))).await;

			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			let body: Value = serde_json::from_slice(calls.borrow()[0].body.as_deref().unwrap()).unwrap();
			assert_eq!(
				body["input"],
				json!([{
					"role": "user",
					"content": [
						{ "type": "input_text", "text": pi_host::js::json_stringify(&context_json()["state"]) },
						{ "type": "input_image", "image_url": "data:image/png;base64,aW1hZ2U=" },
						{ "type": "input_image", "image_url": "data:image/jpeg;base64,aW1hZ2U=" },
					],
				}])
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn rejects_more_than_128_images_before_sending() {
		run(|_| async {
			let (fetch, calls) = recording_fetch(|_| json_response(200, &json!({ "answers": wire_answers() })));
			let mut context_value = context_json();
			context_value["images"] = Value::Array(vec![image(); 129]);
			let context: ClassifierContext = serde_json::from_value(context_value).unwrap();

			let result = classify(&model(), &context, Some(&with_key(fetch))).await;

			assert!(calls.borrow().is_empty());
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			let error_message = result.error_message.unwrap_or_default();
			assert!(error_message.contains("at most 128 images, got 129"), "{error_message}");
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn fails_the_result_when_a_question_is_refused_and_keeps_the_billed_usage() {
		run(|_| async {
			let (fetch, _) = recording_fetch(|_| {
				let answers = wire_answers();
				json_response(
					200,
					&json!({
						"answers": [answers[0], answers[1], { "type": "refusal", "name": "approved" }],
						"usage": wire_usage(),
					}),
				)
			});

			let result = classify(&model(), &context(), Some(&with_key(fetch))).await;

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert_eq!(answers_json(&result), json!({}));
			assert_eq!(
				result.error_message.as_deref(),
				Some("OpenAI Decisions refused to answer approved")
			);
			assert_match_object(
				&serde_json::to_value(result.usage.expect("usage")).unwrap(),
				&json!({ "input": 164 }),
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn returns_missing_and_mistyped_answers_as_classifier_errors() {
		run(|_| async {
			let (missing_fetch, _) =
				recording_fetch(|_| json_response(200, &json!({ "answers": wire_answers()[..2] })));
			let missing = classify(&model(), &context(), Some(&with_key(missing_fetch))).await;
			let (mistyped_fetch, _) = recording_fetch(|_| {
				let answers = wire_answers();
				json_response(
					200,
					&json!({ "answers": [answers[0], answers[1], { "type": "score", "name": "approved" }] }),
				)
			});
			let mistyped = classify(&model(), &context(), Some(&with_key(mistyped_fetch))).await;

			assert_eq!(missing.stop_reason, ClassifierStopReason::Error);
			let missing_message = missing.error_message.unwrap_or_default();
			assert!(
				missing_message.contains("did not return an answer for approved"),
				"{missing_message}"
			);
			assert_eq!(mistyped.stop_reason, ClassifierStopReason::Error);
			let mistyped_message = mistyped.error_message.unwrap_or_default();
			assert!(
				mistyped_message.contains("did not return a predicate answer for approved"),
				"{mistyped_message}"
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn preserves_prototype_sensitive_question_ids_in_answers() {
		run(|_| async {
			let prototype_context: ClassifierContext = serde_json::from_value(json!({
				"state": {},
				"questions": serde_json::from_str::<Value>(
					r#"{"__proto__":{"type":"bool","instructions":"Is this true?","criteria":{"true":"Yes","false":"No"}}}"#,
				)
				.unwrap(),
			}))
			.unwrap();
			let (fetch, _) = recording_fetch(|_| {
				json_response(
					200,
					&json!({ "answers": [{ "type": "predicate", "name": "__proto__", "probability": 0.75 }] }),
				)
			});

			let result = classify(&model(), &prototype_context, Some(&with_key(fetch))).await;

			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert!(result.answers.contains_key("__proto__"));
			assert_eq!(
				answers_json(&result)["__proto__"],
				json!({ "type": "bool", "probability": 0.75 })
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn does_not_retry_gateway_timeouts_and_explains_them_instead_of_returning_the_html_page() {
		run(|_| async {
			let (fetch, calls) = recording_fetch(|_| {
				response(
					504,
					&[("retry-after-ms", "0")],
					"<!DOCTYPE html><html>Gateway time-out</html>",
				)
			});
			// Default retries: the same input would time out again.
			let result = classify(&model(), &context(), Some(&with_key(fetch))).await;

			assert_eq!(calls.borrow().len(), 1);
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			let error_message = result.error_message.unwrap_or_default();
			assert!(
				error_message.contains("OpenAI Decisions error (504): the request timed out at the gateway"),
				"{error_message}"
			);
			assert!(!error_message.contains("<html>"));
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn still_retries_other_server_errors() {
		run(|_| async {
			let attempt = Rc::new(Cell::new(0));
			let counted = attempt.clone();
			let fetch: FetchFunction = Rc::new(move |_| {
				counted.set(counted.get() + 1);
				let response = if counted.get() == 1 {
					response(503, &[("retry-after-ms", "0")], "busy")
				} else {
					json_response(200, &json!({ "answers": wire_answers() }))
				};
				Box::pin(async move { Ok(response) })
			});

			let result = classify(&model(), &context(), Some(&with_key(fetch))).await;

			assert_eq!(attempt.get(), 2);
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn includes_the_api_error_body_for_other_http_failures() {
		run(|_| async {
			let (fetch, _) = recording_fetch(|_| {
				json_response(
					400,
					&json!({ "error": { "message": "Decision input exceeds the token limit.", "type": "invalid_request_error" } }),
				)
			});
			let mut options = with_key(fetch);
			options.max_retries = Some(0.0);

			let result = classify(&model(), &context(), Some(&options)).await;

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			let error_message = result.error_message.unwrap_or_default();
			assert!(
				error_message.contains("OpenAI Decisions error (400)"),
				"{error_message}"
			);
			assert!(
				error_message.contains("Decision input exceeds the token limit."),
				"{error_message}"
			);
		});
	}

	#[test]
	#[ignore = "PORT-STUB: needs api/classifier_shared.rs from w3-bedrock-misc"]
	fn rejects_models_for_other_classifier_apis_and_missing_api_keys() {
		run(|_| async {
			let (fetch, calls) = recording_fetch(|_| json_response(200, &json!({ "answers": wire_answers() })));
			let mut other_model = serde_json::to_value(model()).unwrap();
			other_model["api"] = json!("typesafe-system-one");
			let other_model: ClassifierModel = serde_json::from_value(other_model).unwrap();
			let other_api = classify(&other_model, &context(), Some(&with_key(fetch.clone()))).await;
			let no_key = classify(
				&model(),
				&context(),
				Some(&options(|options| options.fetch = Some(fetch))),
			)
			.await;

			assert!(calls.borrow().is_empty());
			let other_message = other_api.error_message.unwrap_or_default();
			assert!(
				other_message.contains("Unsupported classifier API: typesafe-system-one"),
				"{other_message}"
			);
			let no_key_message = no_key.error_message.unwrap_or_default();
			assert!(
				no_key_message.contains("No API key for provider: openai"),
				"{no_key_message}"
			);
		});
	}
}
