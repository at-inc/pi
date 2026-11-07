//! Port of `packages/ai/test/cloudflare-workers-ai-system-one.test.ts`.
//!
//! Only the catalog checks are ported: `exposes Jev only through classifier catalog accessors` and the compat half
//! of `reads the image form from compat before the model ID`. Every other case classifies through the
//! `cloudflare-workers-ai-system-one` API, which is wave 3 (`w3-bedrock-misc`).

use std::rc::Rc;

use pi_ai::models::create_models;
use pi_ai::providers::all::get_builtin_classifier_model;
use pi_ai::providers::cloudflare_workers_ai::cloudflare_workers_ai_provider;
use pi_ai::types::{AnyModel, ModelType};
use serde_json::{Value, json};

fn setup() -> (Rc<dyn pi_ai::models::MutableModels>, AnyModel) {
	let models = create_models(None);
	models.set_provider(Rc::new(cloudflare_workers_ai_provider()));
	let jev = models
		.get_model_of_type(ModelType::Classifier, "cloudflare-workers-ai", "typesafe/jev")
		.expect("missing Cloudflare Jev model");
	(models, jev)
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

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
	fn reads_the_image_form_from_compat_before_the_model_id() {
		let (models, jev) = setup();
		let clef = models.get_model_of_type(ModelType::Classifier, "cloudflare-workers-ai", "@cf/cloudflare/clef");
		assert_eq!(to_json(&clef)["compat"], json!({ "imageInput": "images" }));
		assert_eq!(to_json(&jev)["compat"], json!({ "imageInput": "assets" }));
	}
}
