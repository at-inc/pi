//! Port of `packages/ai/test/cloudflare-workers-ai-system-one.test.ts`.
//!
//! `vi.fn` fetch mocks are `FetchFunction`s that record each request.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::models::{ModelsClassifierOptions, create_models};
use pi_ai::providers::all::get_builtin_classifier_model;
use pi_ai::providers::cloudflare_workers_ai::cloudflare_workers_ai_provider;
use pi_ai::types::{
	AnyModel, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierStopReason, ModelInputModality,
	ModelType, ProviderRequestOptions,
};
use pi_host::testing::run;
use pi_host::{FetchFunction, HttpRequest, HttpResponse};
use serde_json::{Value, json};

fn context_json() -> Value {
	json!({
		"state": { "message": "Help! My payouts have been failing for 3 days." },
		"questions": {
			"is_urgent": {
				"type": "bool",
				"instructions": "Does this convey urgency?",
				"criteria": { "true": "Explicitly time-sensitive", "false": "No urgency expressed" },
			},
			"department": {
				"type": "choice",
				"instructions": "Which team should handle this?",
				"criteria": { "billing": "Payments", "technical": "Bugs" },
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

/// Model output from https://developers.cloudflare.com/ai/models/typesafe/jev/
fn jev_output() -> Value {
	json!({
		"model": "jev-1.13.0",
		"answers": {
			"is_urgent": { "type": "noul", "noul": 0.95 },
			"department": {
				"type": "choice",
				"choice": "billing",
				"confidence": 0.8,
				"probabilities": { "billing": 0.87, "technical": 0.13 },
			},
		},
		"usage": { "input_tokens": 426, "output_tokens": 73 },
	})
}

/// REST envelope observed from the live /ai/run endpoint.
fn rest_response(state: &str, result: Value) -> Value {
	json!({
		"result": { "state": state, "result": result, "gatewayMetadata": { "keySource": "Unified" } },
		"success": true,
		"errors": [],
		"messages": [],
	})
}

/// Cloudflare-hosted output observed from a live /ai/run call, question ids renamed to match `context`.
/// The envelope carries the output directly, without a run record.
fn clef_output() -> Value {
	json!({
		"model": "clef",
		"answers": {
			"is_urgent": { "type": "noul", "noul": 0.9912 },
			"department": {
				"type": "choice",
				"choice": "technical",
				"probabilities": { "billing": 0.1632, "technical": 0.8368 },
				"confidence": 0.4538,
			},
		},
		"usage": { "input_tokens": 222, "output_tokens": 0 },
	})
}

fn clef_response() -> Value {
	json!({ "result": clef_output(), "success": true, "errors": [], "messages": [] })
}

fn setup() -> (Rc<dyn pi_ai::models::MutableModels>, AnyModel) {
	let models = create_models(None);
	models.set_provider(Rc::new(cloudflare_workers_ai_provider()));
	let jev = models
		.get_model_of_type(ModelType::Classifier, "cloudflare-workers-ai", "typesafe/jev")
		.expect("missing Cloudflare Jev model");
	(models, jev)
}

fn classifier(model: AnyModel) -> ClassifierModel {
	match model {
		AnyModel::Classifier(model) => model,
		_ => panic!("not a classifier model"),
	}
}

fn get_classifier(models: &Rc<dyn pi_ai::models::MutableModels>, id: &str) -> ClassifierModel {
	classifier(
		models
			.get_model_of_type(ModelType::Classifier, "cloudflare-workers-ai", id)
			.unwrap_or_else(|| panic!("missing Cloudflare {id} model")),
	)
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

type Recorded = Rc<RefCell<Vec<HttpRequest>>>;

/// A `vi.fn` fetch that records its requests and answers each with `Response.json(body)`.
fn recording_fetch(response: Value) -> (FetchFunction, Recorded) {
	let requests: Recorded = Rc::new(RefCell::new(Vec::new()));
	let recorded = requests.clone();
	let fetch: FetchFunction = Rc::new(move |request: HttpRequest| {
		recorded.borrow_mut().push(request);
		let body = pi_host::js::json_stringify(&response);
		Box::pin(async move {
			Ok(HttpResponse::from_bytes(
				200,
				IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
				body,
			))
		})
	});
	(fetch, requests)
}

fn body(request: &HttpRequest) -> Value {
	serde_json::from_slice(request.body.as_deref().unwrap()).unwrap()
}

/// `{ apiKey: "cf-key", env: { CLOUDFLARE_ACCOUNT_ID: "account-id" }, fetch }`.
fn auth(fetch: FetchFunction) -> Option<ModelsClassifierOptions> {
	Some(ModelsClassifierOptions {
		base: ClassifierOptions {
			base: ProviderRequestOptions {
				api_key: Some("cf-key".into()),
				env: Some(IndexMap::from([(
					"CLOUDFLARE_ACCOUNT_ID".to_string(),
					"account-id".to_string(),
				)])),
				fetch: Some(fetch),
				..ProviderRequestOptions::default()
			},
			temperature: None,
		},
		..ModelsClassifierOptions::default()
	})
}

fn assert_close(actual: f64, expected: f64) {
	assert!(
		(actual - expected).abs() < 0.005,
		"expected {actual} to be close to {expected}"
	);
}

const RUN_URL: &str = "https://api.cloudflare.com/client/v4/accounts/account-id/ai/run";

mod cloudflare_workers_ai_system_one {
	use super::*;

	#[test]
	fn exposes_jev_only_through_classifier_catalog_accessors() {
		let (models, jev) = setup();
		assert_eq!(
			Some(jev.clone()),
			get_builtin_classifier_model("cloudflare-workers-ai", "typesafe/jev").map(AnyModel::Classifier)
		);
		let jev = to_json(&jev);
		assert_eq!(jev["type"], "classifier");
		assert_eq!(jev["api"], "cloudflare-workers-ai-system-one");
		assert_eq!(models.get_model("cloudflare-workers-ai", "typesafe/jev"), None);
	}

	#[test]
	fn runs_jev_through_the_account_scoped_ai_run_endpoint() {
		run(|_| async {
			let (models, jev) = setup();
			let (fetch, requests) = recording_fetch(rest_response("Completed", jev_output()));

			let result = models.classify(&classifier(jev), &context(), auth(fetch)).await;

			let requests = requests.borrow();
			let payload = body(&requests[0]);
			assert_eq!(payload["model"], "typesafe/jev");
			assert_eq!(payload["input"]["state"], context_json()["state"]);
			assert_eq!(payload["input"]["questions"]["is_urgent"]["type"], "noul");
			assert_eq!(payload["input"]["questions"]["department"]["type"], "choice");
			assert_eq!(requests[0].header("authorization"), Some("Bearer cf-key"));
			assert_eq!(requests[0].url, RUN_URL);
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(
				to_json(&result.answers["is_urgent"]),
				json!({ "type": "bool", "probability": 0.95 })
			);
			let department = to_json(&result.answers["department"]);
			assert_eq!(
				(&department["type"], &department["choice"], &department["confidence"]),
				(&json!("choice"), &json!("billing"), &json!(0.8))
			);
			let usage = result.usage.unwrap();
			assert_eq!((usage.input, usage.output, usage.total_tokens), (426.0, 73.0, 499.0));
		});
	}

	fn runs_through_ai_run_and_parses_its_direct_output(id: &'static str, input_price: f64) {
		run(|_| async move {
			let (models, _) = setup();
			let clef = get_classifier(&models, id);
			let (fetch, requests) = recording_fetch(clef_response());

			let result = models.classify(&clef, &context(), auth(fetch)).await;

			let requests = requests.borrow();
			let payload = body(&requests[0]);
			assert_eq!(payload["model"], id);
			assert_eq!(payload["input"]["state"], context_json()["state"]);
			assert_eq!(payload["input"]["questions"]["is_urgent"]["type"], "noul");
			assert_eq!(requests[0].url, RUN_URL);
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(
				to_json(&result.answers["is_urgent"]),
				json!({ "type": "bool", "probability": 0.9912 })
			);
			let department = to_json(&result.answers["department"]);
			assert_eq!(
				(&department["type"], &department["choice"], &department["confidence"]),
				(&json!("choice"), &json!("technical"), &json!(0.4538))
			);
			let usage = result.usage.unwrap();
			assert_eq!((usage.input, usage.output, usage.total_tokens), (222.0, 0.0, 222.0));
			assert_close(usage.cost.input, (222.0 * input_price) / 1_000_000.0);
		});
	}

	#[test]
	fn runs_cf_cloudflare_clef_through_ai_run_and_parses_its_direct_output() {
		runs_through_ai_run_and_parses_its_direct_output("@cf/cloudflare/clef", 0.24);
	}

	#[test]
	fn runs_cf_cloudflare_clef_flash_through_ai_run_and_parses_its_direct_output() {
		runs_through_ai_run_and_parses_its_direct_output("@cf/cloudflare/clef-flash", 0.038);
	}

	#[test]
	fn runs_cf_cloudflare_clef_omni_through_ai_run_and_parses_its_direct_output() {
		runs_through_ai_run_and_parses_its_direct_output("@cf/cloudflare/clef-omni", 0.15);
	}

	#[test]
	fn sends_images_to_clef_as_data_urls() {
		run(|_| async {
			let (models, _) = setup();
			let clef = get_classifier(&models, "@cf/cloudflare/clef");
			let (fetch, requests) = recording_fetch(clef_response());

			let images = json!([
				{ "type": "image", "data": "cG5n", "mimeType": "image/png" },
				{ "type": "image", "data": "d2VicA==", "mimeType": "image/webp" },
			]);
			let result = models.classify(&clef, &context_with_images(images), auth(fetch)).await;

			let requests = requests.borrow();
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			// Clef rejects `assets` with a 422, so the state stays as is.
			let input = body(&requests[0])["input"].clone();
			let keys: Vec<&String> = input.as_object().unwrap().keys().collect();
			assert_eq!(keys, ["state", "questions", "images"]);
			assert_eq!(input["state"], context_json()["state"]);
			assert!(input["questions"].is_object());
			assert_eq!(
				input["images"],
				json!(["data:image/png;base64,cG5n", "data:image/webp;base64,d2VicA=="])
			);
			assert!(requests[0].header("typesafe-danger-zone-images").is_none());
		});
	}

	#[test]
	fn reads_the_image_form_from_compat_before_the_model_id() {
		run(|_| async {
			let (models, jev) = setup();
			let clef = models.get_model_of_type(ModelType::Classifier, "cloudflare-workers-ai", "@cf/cloudflare/clef");
			assert_eq!(to_json(&clef)["compat"], json!({ "imageInput": "images" }));
			assert_eq!(to_json(&jev)["compat"], json!({ "imageInput": "assets" }));
			let (fetch, requests) = recording_fetch(clef_response());

			// A Clef deployment under an ID the prefix check does not recognize.
			let mut finetune = classifier(jev);
			finetune.id = "@cf/acme/clef-finetune".into();
			finetune.input = vec![ModelInputModality::Text, ModelInputModality::Image];
			finetune.compat = Some(json!({ "imageInput": "images" }).as_object().unwrap().clone());
			let images = json!([{ "type": "image", "data": "cG5n", "mimeType": "image/png" }]);
			models
				.classify(&finetune, &context_with_images(images), auth(fetch))
				.await;

			let input = body(&requests.borrow()[0])["input"].clone();
			assert_eq!(input["images"], json!(["data:image/png;base64,cG5n"]));
			assert!(input.get("assets").is_none());
		});
	}

	#[test]
	fn rejects_more_than_four_images_for_clef_before_sending() {
		run(|_| async {
			let (models, _) = setup();
			let clef = get_classifier(&models, "@cf/cloudflare/clef-flash");
			let (fetch, requests) = recording_fetch(json!({ "result": clef_output(), "success": true }));
			let image = json!({ "type": "image", "data": "cG5n", "mimeType": "image/png" });
			let images = Value::Array(vec![image; 5]);

			let result = models.classify(&clef, &context_with_images(images), auth(fetch)).await;

			assert!(requests.borrow().is_empty());
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Cloudflare Workers AI accepts at most 4 images for @cf/cloudflare/clef-flash, got 5")
			);
		});
	}

	fn sends_images_as_system_one_assets(id: &'static str) {
		run(|_| async move {
			let (models, _) = setup();
			let jev = get_classifier(&models, id);
			let (fetch, requests) = recording_fetch(rest_response("Completed", jev_output()));

			let images = json!([{ "type": "image", "data": "cG5n", "mimeType": "image/png" }]);
			let result = models.classify(&jev, &context_with_images(images), auth(fetch)).await;

			let requests = requests.borrow();
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			let payload = body(&requests[0]);
			assert_eq!(payload["model"], id);
			assert_eq!(payload["input"]["state"], json!(["<image-1>", context_json()["state"]]));
			assert_eq!(
				payload["input"]["assets"],
				json!({ "<image-1>": { "type": "image", "format": "png", "data_base64": "cG5n" } })
			);
			// Workers AI needs no TypeSafe opt-in header.
			assert!(requests[0].header("typesafe-danger-zone-images").is_none());
		});
	}

	#[test]
	fn sends_images_to_typesafe_jev_as_system_one_assets() {
		sends_images_as_system_one_assets("typesafe/jev");
	}

	#[test]
	fn sends_images_to_typesafe_jev_1_14_0_as_system_one_assets() {
		sends_images_as_system_one_assets("typesafe/jev-1.14.0");
	}

	#[test]
	fn reports_runs_that_did_not_complete() {
		run(|_| async {
			let (models, jev) = setup();
			let (fetch, _) = recording_fetch(rest_response("Queued", Value::Null));
			let result = models.classify(&classifier(jev), &context(), auth(fetch)).await;

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert!(
				result
					.error_message
					.unwrap()
					.contains("run did not complete (state: Queued)")
			);
		});
	}

	#[test]
	fn reports_cloudflare_envelope_errors() {
		run(|_| async {
			let (models, jev) = setup();
			let (fetch, _) = recording_fetch(json!({
				"success": false,
				"errors": [{ "code": 5007, "message": "No such model" }],
				"result": null,
			}));
			let result = models.classify(&classifier(jev), &context(), auth(fetch)).await;

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert!(
				result
					.error_message
					.unwrap()
					.contains("Cloudflare Workers AI error: No such model")
			);
		});
	}
}
