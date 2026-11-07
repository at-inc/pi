//! Port of `packages/ai/src/providers/all.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `BuiltinProvider` and the catalog id/API helper types only narrow types; providers and model ids are `&str`.
//!   The getters return copies of the catalog entries, and a provider or model id the catalog lacks reads as `None`
//!   (the TS returns `undefined` despite its types).
//! - `getBuiltinModelDataGeneratedAt` parses `generatedAt` as the `YYYY-MM-DDTHH:mm:ss[.sss]Z` string the generator
//!   writes (`Date.prototype.toISOString`); any other string reads as `Date.parse`'s NaN, so `None`.
//! - `builtinModels` registers each provider as an `Rc<Provider>`.

use std::rc::Rc;
use std::sync::LazyLock;

use serde_json::Value;

use crate::models::{CreateModelsOptions, MutableModels, Provider, create_models};
use crate::models_generated::{CLASSIFIER_MODELS, IMAGE_MODELS, MODELS};
use crate::providers::amazon_bedrock::amazon_bedrock_provider;
use crate::providers::ant_ling::ant_ling_provider;
use crate::providers::anthropic::anthropic_provider;
use crate::providers::azure::azure_provider;
use crate::providers::baseten::baseten_provider;
use crate::providers::cerebras::cerebras_provider;
use crate::providers::cloudflare_ai_gateway::cloudflare_ai_gateway_provider;
use crate::providers::cloudflare_workers_ai::cloudflare_workers_ai_provider;
use crate::providers::deepseek::deepseek_provider;
use crate::providers::fireworks::fireworks_provider;
use crate::providers::github_copilot::github_copilot_provider;
use crate::providers::google::google_provider;
use crate::providers::google_vertex::google_vertex_provider;
use crate::providers::groq::groq_provider;
use crate::providers::huggingface::huggingface_provider;
use crate::providers::kimi_coding::kimi_coding_provider;
use crate::providers::meta::meta_provider;
use crate::providers::minimax::minimax_provider;
use crate::providers::minimax_cn::minimax_cn_provider;
use crate::providers::mistral::mistral_provider;
use crate::providers::moonshotai::moonshotai_provider;
use crate::providers::moonshotai_cn::moonshotai_cn_provider;
use crate::providers::nvidia::nvidia_provider;
use crate::providers::openai::openai_provider;
use crate::providers::openai_codex::openai_codex_provider;
use crate::providers::opencode::opencode_provider;
use crate::providers::opencode_go::opencode_go_provider;
use crate::providers::openrouter::openrouter_provider;
use crate::providers::qwen_token_plan::qwen_token_plan_provider;
use crate::providers::qwen_token_plan_cn::qwen_token_plan_cn_provider;
use crate::providers::qwen_token_plan_individual::qwen_token_plan_individual_provider;
use crate::providers::together::together_provider;
use crate::providers::typesafe::typesafe_provider;
use crate::providers::vercel_ai_gateway::vercel_ai_gateway_provider;
use crate::providers::xai::xai_provider;
use crate::providers::xiaomi::xiaomi_provider;
use crate::providers::xiaomi_token_plan_ams::xiaomi_token_plan_ams_provider;
use crate::providers::xiaomi_token_plan_cn::xiaomi_token_plan_cn_provider;
use crate::providers::xiaomi_token_plan_sgp::xiaomi_token_plan_sgp_provider;
use crate::providers::zai::zai_provider;
use crate::providers::zai_coding_cn::zai_coding_cn_provider;
use crate::types::{AnyModel, ClassifierModel, ImageModel, Model, ModelType};
use crate::utils::model_operations::is_model_type;

pub use crate::providers::radius::radius_provider;

static MODEL_DATA_MANIFEST: LazyLock<Value> = LazyLock::new(|| {
	serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/model-data/.manifest.json")))
		.expect("generated model data .manifest.json is valid JSON")
});

/// Typed read of one generated built-in chat model.
pub fn get_builtin_model(provider: &str, model_id: &str) -> Option<Model> {
	MODELS.get(provider)?.get(model_id).cloned()
}

/// Typed read of one generated built-in image model.
pub fn get_builtin_image_model(provider: &str, model_id: &str) -> Option<ImageModel> {
	IMAGE_MODELS.get(provider)?.get(model_id).cloned()
}

/// Typed read of one generated built-in classifier model.
pub fn get_builtin_classifier_model(provider: &str, model_id: &str) -> Option<ClassifierModel> {
	CLASSIFIER_MODELS.get(provider)?.get(model_id).cloned()
}

pub fn get_builtin_providers() -> Vec<&'static str> {
	MODELS.keys().copied().collect()
}

/// Generation timestamp shared by all built-in provider catalogs.
pub fn get_builtin_model_data_generated_at() -> Option<f64> {
	MODEL_DATA_MANIFEST
		.get("generatedAt")
		.and_then(Value::as_str)
		.and_then(parse_iso_timestamp)
}

/// `Date.parse` of a `toISOString()` timestamp: `YYYY-MM-DDTHH:mm:ss[.sss]Z` in milliseconds since the epoch.
fn parse_iso_timestamp(text: &str) -> Option<f64> {
	let (date, clock) = text.strip_suffix('Z')?.split_once('T')?;
	let number = |text: &str, width: usize| {
		(text.len() == width && text.bytes().all(|byte| byte.is_ascii_digit()))
			.then(|| text.parse::<f64>().ok())
			.flatten()
	};
	let mut date = date.split('-');
	let (year, month, day) = (
		number(date.next()?, 4)?,
		number(date.next()?, 2)?,
		number(date.next()?, 2)?,
	);
	let (clock, millis) = match clock.split_once('.') {
		Some((clock, fraction)) if (1..=3).contains(&fraction.len()) => (clock, number(&format!("{fraction:0<3}"), 3)?),
		Some(_) => return None,
		None => (clock, 0.0),
	};
	let mut clock = clock.split(':');
	let (hour, minute, second) = (
		number(clock.next()?, 2)?,
		number(clock.next()?, 2)?,
		number(clock.next()?, 2)?,
	);
	if date.next().is_some()
		|| clock.next().is_some()
		|| !(1.0..=12.0).contains(&month)
		|| !(1.0..=31.0).contains(&day)
		|| hour > 23.0
		|| minute > 59.0
		|| second > 59.0
	{
		return None;
	}
	// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
	let shifted_year = if month <= 2.0 { year - 1.0 } else { year };
	let era = (shifted_year / 400.0).floor();
	let year_of_era = shifted_year - era * 400.0;
	let month_index = if month > 2.0 { month - 3.0 } else { month + 9.0 };
	let day_of_year = ((153.0 * month_index + 2.0) / 5.0).floor() + day - 1.0;
	let day_of_era = year_of_era * 365.0 + (year_of_era / 4.0).floor() - (year_of_era / 100.0).floor() + day_of_year;
	let days = era * 146_097.0 + day_of_era - 719_468.0;
	Some(days * 86_400_000.0 + ((hour * 60.0 + minute) * 60.0 + second) * 1000.0 + millis)
}

pub fn get_builtin_models(provider: &str) -> Vec<Model> {
	MODELS
		.get(provider)
		.map(|models| models.values().cloned().collect())
		.unwrap_or_default()
}

pub fn get_builtin_image_models(provider: &str) -> Vec<ImageModel> {
	IMAGE_MODELS
		.get(provider)
		.map(|models| models.values().cloned().collect())
		.unwrap_or_default()
}

pub fn get_builtin_classifier_models(provider: &str) -> Vec<ClassifierModel> {
	CLASSIFIER_MODELS
		.get(provider)
		.map(|models| models.values().cloned().collect())
		.unwrap_or_default()
}

pub fn get_all_builtin_models(provider: &str) -> Vec<AnyModel> {
	get_builtin_models(provider)
		.into_iter()
		.map(AnyModel::Chat)
		.chain(get_builtin_image_models(provider).into_iter().map(AnyModel::Image))
		.chain(
			get_builtin_classifier_models(provider)
				.into_iter()
				.map(AnyModel::Classifier),
		)
		.collect()
}

/// All built-in providers, freshly constructed.
pub fn builtin_providers() -> Vec<Provider> {
	vec![
		amazon_bedrock_provider(),
		ant_ling_provider(),
		anthropic_provider(),
		azure_provider(),
		baseten_provider(),
		cerebras_provider(),
		cloudflare_ai_gateway_provider(),
		cloudflare_workers_ai_provider(),
		deepseek_provider(),
		fireworks_provider(),
		github_copilot_provider(),
		google_provider(),
		google_vertex_provider(),
		groq_provider(),
		huggingface_provider(),
		kimi_coding_provider(),
		meta_provider(),
		minimax_provider(),
		minimax_cn_provider(),
		mistral_provider(),
		moonshotai_provider(),
		moonshotai_cn_provider(),
		nvidia_provider(),
		openai_provider(),
		openai_codex_provider(),
		opencode_provider(),
		opencode_go_provider(),
		openrouter_provider(),
		qwen_token_plan_provider(),
		qwen_token_plan_cn_provider(),
		qwen_token_plan_individual_provider(),
		radius_provider(None),
		together_provider(),
		typesafe_provider(),
		vercel_ai_gateway_provider(),
		xai_provider(),
		xiaomi_provider(),
		xiaomi_token_plan_ams_provider(),
		xiaomi_token_plan_cn_provider(),
		xiaomi_token_plan_sgp_provider(),
		zai_provider(),
		zai_coding_cn_provider(),
	]
}

/// A `Models` collection with every built-in provider registered.
pub fn builtin_models(options: Option<CreateModelsOptions>) -> Rc<dyn MutableModels> {
	let models = create_models(options);
	for provider in builtin_providers() {
		models.set_provider(Rc::new(provider));
	}
	models
}

/// All built-in image-generation providers, freshly constructed.
pub fn builtin_images_providers() -> Vec<Provider> {
	builtin_providers()
		.into_iter()
		.filter(|provider| {
			provider.get_all_models.as_ref().is_some_and(|get_all_models| {
				get_all_models()
					.iter()
					.any(|model| is_model_type(model, ModelType::Image))
			})
		})
		.collect()
}
