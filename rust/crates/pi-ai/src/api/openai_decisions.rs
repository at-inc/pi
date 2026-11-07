//! Port of `packages/ai/src/api/openai-decisions.ts`.
//! PORT-STATUS: reviewed
//!
//! `Object.entries` and `Object.fromEntries` follow JS key order through `js_entries`; the parsed probability maps keep
//! insertion order in memory, and `json_stringify` writes them in JS order.

use indexmap::IndexMap;
use pi_host::js::json_stringify;
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::api::classifier_shared::{is_record, parse_classifier_usage, post_classifier_request, required_number};
use crate::types::{
	ClassifierAnswer, ClassifierBoolAnswer, ClassifierChoiceAnswer, ClassifierContext, ClassifierModel,
	ClassifierOptions, ClassifierQuestion, ClassifierResult, ClassifierScoreAnswer, ClassifierStopReason,
};
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::text::js_entries;

// OpenAI's Decisions API: `POST /v1/decisions` with `{ model, input, questions }`.
// https://developers.openai.com/api/docs/guides/decisions
//
// The state is sent as JSON text. With images, the input becomes one user message with the state
// as `input_text` followed by `input_image` data URLs. Questions map to Decisions types:
// `choice` to `choice`, `score` to `score`, and `bool` to `predicate`. Predicates have no
// criteria field, so the meanings of true and false are appended to the instructions.
//
// Only OpenAI API keys work: Sign in with ChatGPT tokens are rejected on this route.

const LABEL: &str = "OpenAI Decisions";

/// The endpoint accepts at most this many image parts per request.
const MAX_IMAGES: usize = 128;

fn predicate_instructions(instructions: &str, when_true: &str, when_false: &str) -> String {
	let meanings: Vec<String> = [
		if when_true.is_empty() {
			String::new()
		} else {
			format!("True means: {when_true}")
		},
		if when_false.is_empty() {
			String::new()
		} else {
			format!("False means: {when_false}")
		},
	]
	.into_iter()
	.filter(|meaning| !meaning.is_empty())
	.collect();
	if meanings.is_empty() {
		instructions.to_string()
	} else {
		format!("{instructions}\n\n{}", meanings.join("\n"))
	}
}

fn wire_question(name: &str, question: &ClassifierQuestion) -> Value {
	match question {
		ClassifierQuestion::Choice(question) => json!({
			"type": "choice",
			"name": name,
			"instructions": question.instructions,
			"choices": js_entries(&question.criteria)
				.into_iter()
				.map(|(value, description)| if description.is_empty() {
					json!({ "value": value })
				} else {
					json!({ "value": value, "description": description })
				})
				.collect::<Vec<_>>(),
		}),
		ClassifierQuestion::Score(question) => json!({
			"type": "score",
			"name": name,
			"instructions": question.instructions,
			"levels": question.criteria.iter().map(|label| json!({ "label": label })).collect::<Vec<_>>(),
		}),
		ClassifierQuestion::Bool(question) => json!({
			"type": "predicate",
			"name": name,
			"instructions": predicate_instructions(&question.instructions, &question.criteria.r#true, &question.criteria.r#false),
		}),
	}
}

fn wire_input(context: &ClassifierContext) -> PiResult<Value> {
	let state = json_stringify(&context.state);
	let images = context.images.as_deref().unwrap_or_default();
	if images.is_empty() {
		return Ok(Value::String(state));
	}
	if images.len() > MAX_IMAGES {
		return Err(PiError::new(format!(
			"{LABEL} accepts at most {MAX_IMAGES} images, got {}",
			images.len()
		)));
	}
	let mut content = vec![json!({ "type": "input_text", "text": state })];
	content.extend(images.iter().map(|image| {
		json!({
			"type": "input_image",
			"image_url": format!("data:{};base64,{}", image.mime_type, image.data),
		})
	}));
	Ok(json!([{ "role": "user", "content": content }]))
}

fn choice_probabilities(value: Option<&Value>, id: &str) -> PiResult<IndexMap<String, f64>> {
	let Some(Value::Array(entries)) = value else {
		return Err(PiError::new(format!("{LABEL} returned invalid probabilities for {id}")));
	};
	let mut probabilities = IndexMap::new();
	for entry in entries {
		let Some(entry_value) = entry.get("value").and_then(Value::as_str).filter(|_| is_record(entry)) else {
			return Err(PiError::new(format!("{LABEL} returned invalid probabilities for {id}")));
		};
		let probability = required_number(
			LABEL,
			entry.get("probability"),
			&format!("probability for {id}.{entry_value}"),
		)?;
		probabilities.insert(entry_value.to_string(), probability);
	}
	Ok(probabilities)
}

fn parse_answer(id: &str, question: &ClassifierQuestion, answer: &Map<String, Value>) -> PiResult<ClassifierAnswer> {
	let answer_type = answer.get("type").and_then(Value::as_str);
	if answer_type == Some("refusal") {
		return Err(PiError::new(format!("{LABEL} refused to answer {id}")));
	}
	match question {
		ClassifierQuestion::Choice(_) => {
			let (Some("choice"), Some(Value::String(choice))) = (answer_type, answer.get("choice")) else {
				return Err(PiError::new(format!("{LABEL} did not return a choice answer for {id}")));
			};
			Ok(ClassifierAnswer::Choice(ClassifierChoiceAnswer {
				choice: choice.clone(),
				probabilities: choice_probabilities(answer.get("probabilities"), id)?,
				confidence: required_number(LABEL, answer.get("confidence"), &format!("confidence for {id}"))?,
			}))
		}
		ClassifierQuestion::Score(_) => {
			if answer_type != Some("score") {
				return Err(PiError::new(format!("{LABEL} did not return a score answer for {id}")));
			}
			Ok(ClassifierAnswer::Score(ClassifierScoreAnswer {
				score: required_number(LABEL, answer.get("score"), &format!("score for {id}"))?,
				confidence: required_number(LABEL, answer.get("confidence"), &format!("confidence for {id}"))?,
			}))
		}
		ClassifierQuestion::Bool(_) => {
			if answer_type != Some("predicate") {
				return Err(PiError::new(format!(
					"{LABEL} did not return a predicate answer for {id}"
				)));
			}
			Ok(ClassifierAnswer::Bool(ClassifierBoolAnswer {
				probability: required_number(LABEL, answer.get("probability"), &format!("probability for {id}"))?,
			}))
		}
	}
}

fn parse_answers(value: Option<&Value>, context: &ClassifierContext) -> PiResult<IndexMap<String, ClassifierAnswer>> {
	let Some(Value::Array(answers)) = value else {
		return Err(PiError::new(format!("{LABEL} returned an unexpected response")));
	};
	let mut by_name: IndexMap<&str, &Map<String, Value>> = IndexMap::new();
	for answer in answers {
		if let Value::Object(record) = answer
			&& let Some(name) = record.get("name").and_then(Value::as_str)
		{
			by_name.insert(name, record);
		}
	}
	let mut parsed = IndexMap::new();
	for (id, question) in js_entries(&context.questions) {
		let Some(answer) = by_name.get(id.as_str()) else {
			return Err(PiError::new(format!("{LABEL} did not return an answer for {id}")));
		};
		parsed.insert(id.clone(), parse_answer(id, question, answer)?);
	}
	Ok(parsed)
}

/// Cloudflare in front of api.openai.com answers 504 with an HTML page when a request runs longer
/// than about five seconds. Large inputs, currently above roughly 600K tokens, hit this limit, and
/// retrying the same input runs into it again, so 504 is not retried.
const NO_RETRY_STATUSES: [u16; 1] = [504];

fn error_message(error: &PiError) -> String {
	if error.status == Some(504) {
		return format!(
			"{LABEL} error (504): the request timed out at the gateway. Very large inputs (above roughly 600K tokens) currently exceed its time limit."
		);
	}
	format_provider_error(&normalize_provider_error(error), Some(&format!("{LABEL} error")))
}

/// Classification through OpenAI's Decisions API.
pub async fn classify(
	model: &ClassifierModel,
	context: &ClassifierContext,
	options: Option<&ClassifierOptions>,
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

	let result: PiResult<()> = async {
		if model.api != "openai-decisions" {
			return Err(PiError::new(format!("Unsupported classifier API: {}", model.api)));
		}
		let url = url::Url::parse(&format!("{}/", model.base_url.trim_end_matches('/')))
			.and_then(|base| base.join("decisions"))
			.map_err(|_| PiError::type_error("Invalid URL"))?;
		let questions: Vec<Value> = js_entries(&context.questions)
			.into_iter()
			.map(|(id, question)| wire_question(id, question))
			.collect();
		let body = post_classifier_request(
			LABEL,
			url.as_str(),
			model,
			json!({ "model": model.id, "input": wire_input(context)?, "questions": questions }),
			options,
			Some(&NO_RETRY_STATUSES),
		)
		.await?;
		if !is_record(&body) {
			return Err(PiError::new(format!("{LABEL} returned an unexpected response")));
		}
		// Set before parsing answers: a request with malformed or refused answers was still billed.
		if let Some(usage) = parse_classifier_usage(body.get("usage"), model) {
			output.usage = Some(usage);
		}
		output.answers = parse_answers(body.get("answers"), context)?;
		Ok(())
	}
	.await;
	if let Err(error) = result {
		output.answers = IndexMap::new();
		output.stop_reason = if options
			.and_then(|options| options.signal.as_ref())
			.is_some_and(|signal| signal.aborted())
		{
			ClassifierStopReason::Aborted
		} else {
			ClassifierStopReason::Error
		};
		output.error_message = Some(error_message(&error));
	}
	output
}
