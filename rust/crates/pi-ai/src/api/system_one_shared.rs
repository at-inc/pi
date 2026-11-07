//! Port of `packages/ai/src/api/system-one-shared.ts`.
//! PORT-STATUS: ported
//!
//! `SystemOneWireRequest` is built as a JSON object in the TS insertion order (`state`, `questions`, then `assets` or
//! `images`), because it is sent as-is. Questions serialize from the typed `ClassifierQuestion`, so their keys follow
//! the Rust struct order (`type`, `instructions`, `criteria`) and app fields on a question are not sent. Question IDs,
//! probabilities and answers are walked in JS `Object.entries` order (array-index keys first). `SystemOneCompat`
//! fields read through `ClassifierModel::system_one_compat`, so an `imageInput` value outside the three known forms
//! reads as unset and falls back to the detected form, where TS would send `assets`.

use indexmap::IndexMap;
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};
use url::Url;

use super::classifier_shared::{parse_classifier_usage, post_classifier_request, required_number};
use crate::providers::compat_schema::SystemOneImageInput;
use crate::types::{
	ClassifierAnswer, ClassifierBoolAnswer, ClassifierChoiceAnswer, ClassifierContext, ClassifierModel,
	ClassifierOptions, ClassifierQuestion, ClassifierResult, ClassifierScoreAnswer, ClassifierStopReason, ImageContent,
	ModelInputModality, ProviderHeaders,
};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::text::js_entries;

/// TypeSafe System One request body without the transport-specific envelope: `state` (the context state, or with
/// images a list of the images or their asset names followed by the state), `questions`, and optionally `assets`
/// (images as System One assets, keyed by the names the state references) or `images` (images as data URLs, Clef's
/// extension to System One).
pub type SystemOneWireRequest = Map<String, Value>;

/// System One compatibility settings with every field resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSystemOneCompat {
	pub image_input: SystemOneImageInput,
	pub send_image_opt_in_header: bool,
}

/// TypeSafe's opt-in header for its experimental image input.
const IMAGE_OPT_IN_HEADER: &str = "TypeSafe-Danger-Zone-Images";

/// Differences between services that serve System One models.
pub struct SystemOneTransport {
	/// Classifier API implemented by this transport.
	pub api: &'static str,
	/// Service name used in error messages.
	pub label: &'static str,
	/// Compatibility settings for models that do not set them in `compat`.
	pub detect_compat: fn(&ClassifierModel) -> ResolvedSystemOneCompat,
	/// Absolute request URL.
	pub url: fn(&ClassifierModel) -> PiResult<Url>,
	/// Wraps the System One request in the service's request envelope.
	pub payload: fn(&ClassifierModel, SystemOneWireRequest) -> Value,
	/// Extracts the System One output (`{ answers, usage }`) from the service's response envelope.
	pub output: fn(Value) -> PiResult<Map<String, Value>>,
}

/// `new URL(path, base)` where `base` is `baseUrl` with its trailing slashes replaced by one. The TS `URL`
/// constructor throws `TypeError: Invalid URL` for a base it cannot parse.
pub(crate) fn join_base_url(base_url: &str, path: &str) -> PiResult<Url> {
	Url::parse(&format!("{}/", base_url.trim_end_matches('/')))
		.and_then(|base| base.join(path))
		.map_err(|_| PiError::type_error("Invalid URL"))
}

fn probabilities(label: &str, value: Option<&Value>, id: &str) -> PiResult<IndexMap<String, f64>> {
	let Some(Value::Object(value)) = value else {
		return Err(PiError::new(format!("{label} returned invalid probabilities for {id}")));
	};
	let value: IndexMap<String, Value> = value.clone().into_iter().collect();
	js_entries(&value)
		.into_iter()
		.map(|(key, probability)| {
			Ok((
				key.clone(),
				required_number(label, Some(probability), &format!("probability for {id}.{key}"))?,
			))
		})
		.collect()
}

fn parse_answers(
	label: &str,
	value: Option<&Value>,
	context: &ClassifierContext,
) -> PiResult<IndexMap<String, ClassifierAnswer>> {
	let Some(Value::Object(value)) = value else {
		return Err(PiError::new(format!("{label} returned an unexpected response")));
	};
	let mut answers = IndexMap::new();
	for (id, question) in js_entries(&context.questions) {
		let Some(Value::Object(answer)) = value.get(id.as_str()) else {
			return Err(PiError::new(format!("{label} did not return an answer for {id}")));
		};
		let answer_type = answer.get("type").and_then(Value::as_str);
		let parsed = match question {
			ClassifierQuestion::Choice(_) => {
				let (Some("choice"), Some(Value::String(choice))) = (answer_type, answer.get("choice")) else {
					return Err(PiError::new(format!("{label} did not return a choice answer for {id}")));
				};
				ClassifierAnswer::Choice(ClassifierChoiceAnswer {
					choice: choice.clone(),
					probabilities: probabilities(label, answer.get("probabilities"), id)?,
					confidence: required_number(label, answer.get("confidence"), &format!("confidence for {id}"))?,
				})
			}
			ClassifierQuestion::Score(_) => {
				if answer_type != Some("score") {
					return Err(PiError::new(format!("{label} did not return a score answer for {id}")));
				}
				ClassifierAnswer::Score(ClassifierScoreAnswer {
					score: required_number(label, answer.get("score"), &format!("score for {id}"))?,
					confidence: required_number(label, answer.get("confidence"), &format!("confidence for {id}"))?,
				})
			}
			ClassifierQuestion::Bool(_) => {
				if answer_type != Some("noul") {
					return Err(PiError::new(format!("{label} did not return a bool answer for {id}")));
				}
				ClassifierAnswer::Bool(ClassifierBoolAnswer {
					probability: required_number(label, answer.get("noul"), &format!("probability for {id}"))?,
				})
			}
		};
		answers.insert(id.clone(), parsed);
	}
	Ok(answers)
}

/// Clef accepts at most this many images per request.
const MAX_DATA_URL_IMAGES: usize = 4;

fn data_url(image: &ImageContent) -> String {
	format!("data:{};base64,{}", image.mime_type, image.data)
}

/// Adds the context images to `request` in the service's form.
fn add_images(
	label: &str,
	model: &ClassifierModel,
	input: SystemOneImageInput,
	images: &[ImageContent],
	request: &mut SystemOneWireRequest,
) -> PiResult<()> {
	let state = request.get("state").cloned().unwrap_or(Value::Null);
	match input {
		SystemOneImageInput::Images => {
			if images.len() > MAX_DATA_URL_IMAGES {
				return Err(PiError::new(format!(
					"{label} accepts at most {MAX_DATA_URL_IMAGES} images for {}, got {}",
					model.id,
					images.len()
				)));
			}
			let urls = images
				.iter()
				.map(|image| {
					if !matches!(image.mime_type.as_str(), "image/png" | "image/jpeg" | "image/webp") {
						return Err(PiError::new(format!(
							"{label} accepts PNG, JPEG, and WebP images for {}, got {}",
							model.id, image.mime_type
						)));
					}
					Ok(Value::String(data_url(image)))
				})
				.collect::<PiResult<Vec<_>>>()?;
			request.insert("images".into(), Value::Array(urls));
		}
		SystemOneImageInput::StateImageUrls => {
			// OpenRouter validates the images against the model that serves the request.
			let mut parts: Vec<Value> = images
				.iter()
				.map(|image| json!({ "type": "image_url", "image_url": { "url": data_url(image) } }))
				.collect();
			parts.push(state);
			request.insert("state".into(), Value::Array(parts));
		}
		SystemOneImageInput::Assets => {
			let mut assets = Map::new();
			for (index, image) in images.iter().enumerate() {
				let format = match image.mime_type.as_str() {
					"image/png" => "png",
					"image/jpeg" => "jpeg",
					_ => {
						return Err(PiError::new(format!(
							"{label} accepts PNG and JPEG images for {}, got {}",
							model.id, image.mime_type
						)));
					}
				};
				assets.insert(
					format!("<image-{}>", index + 1),
					json!({ "type": "image", "format": format, "data_base64": image.data }),
				);
			}
			// Every asset must be referenced from the request. The names go before the state, where Clef
			// also places its images.
			let mut names: Vec<Value> = assets.keys().cloned().map(Value::String).collect();
			names.push(state);
			request.insert("assets".into(), Value::Object(assets));
			request.insert("state".into(), Value::Array(names));
		}
	}
	Ok(())
}

/// Maps public `bool` questions to TypeSafe's wire-level `noul` type.
fn wire_request(context: &ClassifierContext) -> SystemOneWireRequest {
	let questions: Map<String, Value> = js_entries(&context.questions)
		.into_iter()
		.map(|(id, question)| {
			let mut value = serde_json::to_value(question).expect("classifier questions are plain JSON");
			if let (ClassifierQuestion::Bool(_), Value::Object(fields)) = (question, &mut value) {
				fields.insert("type".into(), Value::String("noul".into()));
			}
			(id.clone(), value)
		})
		.collect();
	let mut request = Map::new();
	request.insert("state".into(), Value::Object(context.state.clone()));
	request.insert("questions".into(), Value::Object(questions));
	request
}

/// Runs one System One classification over the given transport. Failures are encoded in the result.
pub async fn classify_system_one(
	transport: &SystemOneTransport,
	model: &ClassifierModel,
	context: &ClassifierContext,
	options: Option<ClassifierOptions>,
) -> ClassifierResult {
	let mut output = ClassifierResult {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		answers: IndexMap::new(),
		usage: None,
		stop_reason: ClassifierStopReason::Stop,
		error_message: None,
		timestamp: pi_host::now_ms(),
	};

	let Err(error) = run_system_one(transport, model, context, options.as_ref(), &mut output).await else {
		return output;
	};
	let aborted = options
		.as_ref()
		.and_then(|options| options.signal.as_ref())
		.is_some_and(|signal| signal.aborted());
	output.stop_reason = if aborted {
		ClassifierStopReason::Aborted
	} else {
		ClassifierStopReason::Error
	};
	let prefix = format!("{} error", transport.label);
	output.error_message = Some(format_provider_error(&normalize_provider_error(&error), Some(&prefix)));
	output
}

/// The `try` body of `classifySystemOne`.
async fn run_system_one(
	transport: &SystemOneTransport,
	model: &ClassifierModel,
	context: &ClassifierContext,
	options: Option<&ClassifierOptions>,
	output: &mut ClassifierResult,
) -> PiResult<()> {
	if *model.api != *transport.api {
		return Err(PiError::new(format!("Unsupported classifier API: {}", model.api)));
	}
	let mut request = wire_request(context);
	let mut image_options: Option<ClassifierOptions> = None;
	if let Some(images) = context.images.as_ref().filter(|images| !images.is_empty()) {
		if !model.input.contains(&ModelInputModality::Image) {
			return Err(PiError::new(format!(
				"{} does not support image input for {}",
				transport.label, model.id
			)));
		}
		let detected = (transport.detect_compat)(model);
		let compat = model.system_one_compat();
		add_images(
			transport.label,
			model,
			compat.image_input.unwrap_or(detected.image_input),
			images,
			&mut request,
		)?;
		if compat
			.send_image_opt_in_header
			.unwrap_or(detected.send_image_opt_in_header)
		{
			let mut headers = ProviderHeaders::from([(IMAGE_OPT_IN_HEADER.to_string(), Some("true".to_string()))]);
			if let Some(option_headers) = options.and_then(|options| options.headers.as_ref()) {
				headers.extend(option_headers.clone());
			}
			let mut next = options.cloned().unwrap_or_default();
			next.headers = Some(headers);
			image_options = Some(next);
		}
	}
	let url = (transport.url)(model)?;
	let payload = (transport.payload)(model, request);
	let body = post_classifier_request(
		transport.label,
		&url,
		model,
		payload,
		image_options.as_ref().or(options),
		None,
	)
	.await?;
	let result = (transport.output)(body)?;
	// Set before parsing answers: a request with malformed answers was still billed.
	if let Some(usage) = parse_classifier_usage(result.get("usage"), model) {
		output.usage = Some(usage);
	}
	output.answers = parse_answers(transport.label, result.get("answers"), context)?;
	Ok(())
}
