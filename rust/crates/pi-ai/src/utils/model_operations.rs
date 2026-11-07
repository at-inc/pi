//! Port of `packages/ai/src/utils/model-operations.ts`.
//! PORT-STATUS: ported
//!
//! TS reads `model.type`, `model.id` and `model.provider` off any model object. The port reads them through
//! [`ModelRef`], implemented by `Model`, `ImageModel`, `ClassifierModel` and `AnyModel`. `getModelType` returns
//! the type as a string, because a stored model may carry a type this version does not know (`AnyModel::Other`).

use indexmap::IndexMap;

use pi_host::PiResult;

use crate::types::{
	AnyModel, AssistantImages, ClassifierContext, ClassifierModel, ClassifierResult, ClassifierStopReason, ImageModel,
	ImagesStopReason, Model, ModelInputModality, ModelType,
};
use crate::utils::diagnostics::{Thrown, format_thrown_value};
use crate::utils::models_error::{ModelsError, ModelsErrorCode};

/// The fields every model operation reads, whatever the model's type.
pub trait ModelRef {
	/// `model.type` as written, or `None` when absent or `null`.
	fn model_type_field(&self) -> Option<&str>;
	/// `model.id`.
	fn model_id(&self) -> &str;
	/// `model.provider`.
	fn model_provider(&self) -> &str;
}

/// The string value of a `ModelType`.
pub fn model_type_name(model_type: ModelType) -> &'static str {
	match model_type {
		ModelType::Chat => "chat",
		ModelType::Image => "image",
		ModelType::Classifier => "classifier",
	}
}

impl ModelRef for Model {
	fn model_type_field(&self) -> Option<&str> {
		self.r#type.map(model_type_name)
	}

	fn model_id(&self) -> &str {
		&self.id
	}

	fn model_provider(&self) -> &str {
		&self.provider
	}
}

impl ModelRef for ImageModel {
	fn model_type_field(&self) -> Option<&str> {
		Some(model_type_name(self.r#type))
	}

	fn model_id(&self) -> &str {
		&self.id
	}

	fn model_provider(&self) -> &str {
		&self.provider
	}
}

impl ModelRef for ClassifierModel {
	fn model_type_field(&self) -> Option<&str> {
		Some(model_type_name(self.r#type))
	}

	fn model_id(&self) -> &str {
		&self.id
	}

	fn model_provider(&self) -> &str {
		&self.provider
	}
}

impl ModelRef for AnyModel {
	fn model_type_field(&self) -> Option<&str> {
		match self {
			AnyModel::Chat(model) => model.model_type_field(),
			AnyModel::Image(model) => model.model_type_field(),
			AnyModel::Classifier(model) => model.model_type_field(),
			AnyModel::Other(value) => value.get("type").and_then(|value| value.as_str()),
		}
	}

	fn model_id(&self) -> &str {
		match self {
			AnyModel::Chat(model) => &model.id,
			AnyModel::Image(model) => &model.id,
			AnyModel::Classifier(model) => &model.id,
			AnyModel::Other(value) => value.get("id").and_then(|value| value.as_str()).unwrap_or(""),
		}
	}

	fn model_provider(&self) -> &str {
		match self {
			AnyModel::Chat(model) => &model.provider,
			AnyModel::Image(model) => &model.provider,
			AnyModel::Classifier(model) => &model.provider,
			AnyModel::Other(value) => value.get("provider").and_then(|value| value.as_str()).unwrap_or(""),
		}
	}
}

/// The type of a model. Models without `type` are chat models.
pub fn get_model_type<M: ModelRef + ?Sized>(model: &M) -> &str {
	model.model_type_field().unwrap_or("chat")
}

/// Runtime-checked model type narrowing, including legacy chat models without `type`.
pub fn is_model_type<M: ModelRef + ?Sized>(model: &M, model_type: ModelType) -> bool {
	get_model_type(model) == model_type_name(model_type)
}

pub fn assert_chat_model<M: ModelRef + ?Sized>(model: &M) -> PiResult<()> {
	if !is_model_type(model, ModelType::Chat) {
		return Err(ModelsError::new(
			ModelsErrorCode::Provider,
			&format!(
				"Model {}/{} is not a chat model",
				model.model_provider(),
				model.model_id()
			),
			None,
		));
	}
	Ok(())
}

pub fn assert_image_model<M: ModelRef + ?Sized>(model: &M) -> PiResult<()> {
	if !is_model_type(model, ModelType::Image) {
		return Err(ModelsError::new(
			ModelsErrorCode::Provider,
			&format!(
				"Model {}/{} is not an image model",
				model.model_provider(),
				model.model_id()
			),
			None,
		));
	}
	Ok(())
}

pub fn assert_classifier_model<M: ModelRef + ?Sized>(model: &M) -> PiResult<()> {
	if !is_model_type(model, ModelType::Classifier) {
		return Err(ModelsError::new(
			ModelsErrorCode::Provider,
			&format!(
				"Model {}/{} is not a classifier model",
				model.model_provider(),
				model.model_id()
			),
			None,
		));
	}
	Ok(())
}

/// Rejects classifier images for models whose catalog entry does not accept image input.
pub fn assert_classifier_input_supported(model: &ClassifierModel, context: &ClassifierContext) -> PiResult<()> {
	let has_images = context.images.as_ref().is_some_and(|images| !images.is_empty());
	if has_images && !model.input.contains(&ModelInputModality::Image) {
		return Err(ModelsError::new(
			ModelsErrorCode::Provider,
			&format!("Model {}/{} does not accept image input", model.provider, model.id),
			None,
		));
	}
	Ok(())
}

/// `error instanceof Error ? error.message : String(error)`.
fn thrown_message(error: Thrown<'_>) -> String {
	match error {
		Thrown::Error(error) => error.message.clone(),
		value @ Thrown::Value(_) => format_thrown_value(value),
	}
}

pub fn image_error_result<'a>(model: &ImageModel, error: impl Into<Thrown<'a>>, aborted: bool) -> AssistantImages {
	AssistantImages {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		output: Vec::new(),
		response_id: None,
		usage: None,
		stop_reason: if aborted {
			ImagesStopReason::Aborted
		} else {
			ImagesStopReason::Error
		},
		error_message: Some(thrown_message(error.into())),
		timestamp: pi_host::now_ms(),
	}
}

pub fn classifier_error_result<'a>(
	model: &ClassifierModel,
	error: impl Into<Thrown<'a>>,
	aborted: bool,
) -> ClassifierResult {
	ClassifierResult {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		answers: IndexMap::new(),
		usage: None,
		stop_reason: if aborted {
			ClassifierStopReason::Aborted
		} else {
			ClassifierStopReason::Error
		},
		error_message: Some(thrown_message(error.into())),
		timestamp: pi_host::now_ms(),
	}
}
