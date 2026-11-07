//! Port of `packages/ai/src/api/google-shared.ts`.
//! PORT-STATUS: ported
//!
//! Gemini `Content` and `Part` values, and the tool declarations, are JSON values built in the TS object-literal key
//! order; they are what `onPayload` sees, before the client converts them to the wire format. The SDK enums
//! (`FinishReason`, `FunctionCallingConfigMode`, `ThinkingLevel`) live in `google_client.rs`. `mapStopReason` takes the
//! raw `finishReason` string, because the SDK passes the response value through untyped: a value outside the SDK enum
//! reaches the exhaustive switch's `default` and throws `Unhandled stop reason`, as in TS. `resolveGoogleThinkingLevel`
//! takes a `ModelThinkingLevel` (the TS `ThinkingLevel` plus `off`), because its callers pass clamped levels.
//! `sanitizeSurrogates` is the identity in Rust (see `utils/sanitize_unicode.rs`). `retryGoogleRequest` adds the
//! missing `headers` to the error's `data` object (or creates one); an error whose `data` is not an object has no
//! property bag to hold it and is rethrown as is.

use std::future::Future;
use std::sync::LazyLock;

use pi_host::js::utf16_len;
use pi_host::{AbortSignal, PiResult};
use regex::Regex;
use serde_json::{Map, Value, json};

use super::constrained_sampling::{get_json_schema_tool_parameters, resolve_json_schema_strict_sampling};
use super::google_client::{FunctionCallingConfigMode, ThinkingLevel as GoogleSdkThinkingLevel};
use super::transform_messages::transform_messages;
use crate::models::clamp_thinking_level;
use crate::types::{
	AssistantContent, AssistantMessage, Message, Model, ModelInputModality, ModelThinkingLevel, StopReason, Tool,
	TranscriptContext, UserContent, UserMessageContent,
};
use crate::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::text::js_trim;
use crate::utils::transcript::{collapse_system_messages, without_initial_system_message};

/// Thinking level for Gemini 3 models.
/// Mirrors Google's ThinkingLevel enum values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoogleApiThinkingLevel {
	ThinkingLevelUnspecified,
	Minimal,
	Low,
	Medium,
	High,
}

impl GoogleApiThinkingLevel {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			GoogleApiThinkingLevel::ThinkingLevelUnspecified => "THINKING_LEVEL_UNSPECIFIED",
			GoogleApiThinkingLevel::Minimal => "MINIMAL",
			GoogleApiThinkingLevel::Low => "LOW",
			GoogleApiThinkingLevel::Medium => "MEDIUM",
			GoogleApiThinkingLevel::High => "HIGH",
		}
	}
}

/// `Exclude<ThinkingLevel, "xhigh" | "max">`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResolvedGoogleThinkingLevel {
	Minimal,
	Low,
	Medium,
	High,
}

impl ResolvedGoogleThinkingLevel {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			ResolvedGoogleThinkingLevel::Minimal => "minimal",
			ResolvedGoogleThinkingLevel::Low => "low",
			ResolvedGoogleThinkingLevel::Medium => "medium",
			ResolvedGoogleThinkingLevel::High => "high",
		}
	}
}

fn google_sdk_thinking_level_map(level: GoogleApiThinkingLevel) -> GoogleSdkThinkingLevel {
	match level {
		GoogleApiThinkingLevel::ThinkingLevelUnspecified => GoogleSdkThinkingLevel::ThinkingLevelUnspecified,
		GoogleApiThinkingLevel::Minimal => GoogleSdkThinkingLevel::Minimal,
		GoogleApiThinkingLevel::Low => GoogleSdkThinkingLevel::Low,
		GoogleApiThinkingLevel::Medium => GoogleSdkThinkingLevel::Medium,
		GoogleApiThinkingLevel::High => GoogleSdkThinkingLevel::High,
	}
}

/// Resolve a supported pi level or model-specific Google mapping to a standard Google level.
pub fn resolve_google_thinking_level(
	model: &Model,
	level: ModelThinkingLevel,
) -> PiResult<ResolvedGoogleThinkingLevel> {
	let mapped = model.thinking_level_map.as_ref().and_then(|map| map.get(level));
	let resolved_level = match mapped {
		Some(Some(mapped)) => mapped.to_lowercase(),
		_ => level.as_str().to_string(),
	};
	match resolved_level.as_str() {
		"minimal" => Ok(ResolvedGoogleThinkingLevel::Minimal),
		"low" => Ok(ResolvedGoogleThinkingLevel::Low),
		"medium" => Ok(ResolvedGoogleThinkingLevel::Medium),
		"high" => Ok(ResolvedGoogleThinkingLevel::High),
		_ => {
			let mapped = match mapped {
				None => "undefined",
				Some(None) => "null",
				Some(Some(mapped)) => mapped,
			};
			Err(pi_host::PiError::new(format!(
				"Unsupported Google thinking level mapping for {}/{}: {} -> {mapped}",
				&*model.provider,
				model.id,
				level.as_str()
			)))
		}
	}
}

static GEMINI_3_PRO_OR_FLASH: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"gemini-3(?:\.[0-9]+)?-(?:pro|flash)").expect("valid regex"));
static GEMINI_4_OR_LATER: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"gemini-(?:[4-9]|[1-9][0-9])").expect("valid regex"));
static GEMMA_4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"gemma-?4").expect("valid regex"));

/// Whether this model uses Gemini's discrete `thinkingLevel` control instead of
/// the token-based `thinkingBudget` control. Supported levels come from the
/// model's `thinkingLevelMap`; this only selects the Google wire format.
pub fn uses_google_thinking_level(model: &Model) -> bool {
	let id = model.id.to_lowercase();
	// Match Gemini 3 Pro/Flash IDs with or without a minor version, such as
	// gemini-3-flash-preview, gemini-3.1-pro-preview, and gemini-3.8-flash.
	GEMINI_3_PRO_OR_FLASH.is_match(&id)
		// Match Gemini 4 and later, such as gemini-4-argon (#10890).
		|| GEMINI_4_OR_LATER.is_match(&id)
		|| id == "gemini-flash-latest"
		|| id == "gemini-flash-lite-latest"
		// Match both hosted Gemma 4 naming forms: gemma-4-* and gemma4-*.
		|| GEMMA_4.is_match(&id)
}

pub fn to_google_thinking_level(level: ResolvedGoogleThinkingLevel) -> GoogleApiThinkingLevel {
	match level {
		ResolvedGoogleThinkingLevel::Minimal => GoogleApiThinkingLevel::Minimal,
		ResolvedGoogleThinkingLevel::Low => GoogleApiThinkingLevel::Low,
		ResolvedGoogleThinkingLevel::Medium => GoogleApiThinkingLevel::Medium,
		ResolvedGoogleThinkingLevel::High => GoogleApiThinkingLevel::High,
	}
}

pub fn to_google_sdk_thinking_level(level: GoogleApiThinkingLevel) -> GoogleSdkThinkingLevel {
	google_sdk_thinking_level_map(level)
}

/// The `ThinkingConfig` that disables thinking: `{ thinkingBudget: 0 }` or the lowest supported `thinkingLevel`.
pub fn get_disabled_google_thinking_config(model: &Model) -> PiResult<Map<String, Value>> {
	if !uses_google_thinking_level(model) {
		return Ok(Map::from_iter([("thinkingBudget".to_string(), json!(0.0))]));
	}

	let fallback = clamp_thinking_level(model, ModelThinkingLevel::Off);
	if fallback == ModelThinkingLevel::Off {
		return Ok(Map::from_iter([("thinkingBudget".to_string(), json!(0.0))]));
	}

	let resolved_level = resolve_google_thinking_level(model, fallback)?;
	let api_level = to_google_thinking_level(resolved_level);
	Ok(Map::from_iter([(
		"thinkingLevel".to_string(),
		Value::String(to_google_sdk_thinking_level(api_level).as_str().to_string()),
	)]))
}

/// Determines whether a streamed Gemini `Part` should be treated as "thinking".
///
/// Protocol note (Gemini / Vertex AI thought signatures):
/// - `thought: true` is the definitive marker for thinking content (thought summaries).
/// - `thoughtSignature` is an encrypted representation of the model's internal thought process
///   used to preserve reasoning context across multi-turn interactions.
/// - `thoughtSignature` can appear on ANY part type (text, functionCall, etc.) - it does NOT
///   indicate the part itself is thinking content.
/// - For non-functionCall responses, the signature appears on the last part for context replay.
/// - When persisting/replaying model outputs, signature-bearing parts must be preserved as-is;
///   do not merge/move signatures across parts.
///
/// See: https://ai.google.dev/gemini-api/docs/thought-signatures
pub fn is_thinking_part(part: &Value) -> bool {
	part.get("thought") == Some(&Value::Bool(true))
}

/// Retain thought signatures during streaming.
///
/// Some backends only send `thoughtSignature` on the first delta for a given part/block; later deltas may omit it.
/// This helper preserves the last non-empty signature for the current block.
///
/// Note: this does NOT merge or move signatures across distinct response parts. It only prevents
/// a signature from being overwritten with `undefined` within the same streamed block.
pub fn retain_thought_signature(existing: Option<String>, incoming: Option<&str>) -> Option<String> {
	match incoming {
		Some(incoming) if !incoming.is_empty() => Some(incoming.to_string()),
		_ => existing,
	}
}

// Thought signatures must be base64 for Google APIs (TYPE_BYTES).
static BASE64_SIGNATURE_PATTERN: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[A-Za-z0-9+/]+={0,2}$").expect("valid regex"));

fn is_valid_thought_signature(signature: Option<&str>) -> bool {
	let Some(signature) = signature.filter(|signature| !signature.is_empty()) else {
		return false;
	};
	if !utf16_len(signature).is_multiple_of(4) {
		return false;
	}
	BASE64_SIGNATURE_PATTERN.is_match(signature)
}

/// Only keep signatures from the same provider/model and with valid base64.
fn resolve_thought_signature(is_same_provider_and_model: bool, signature: Option<&str>) -> Option<String> {
	if is_same_provider_and_model && is_valid_thought_signature(signature) {
		signature.map(str::to_string)
	} else {
		None
	}
}

/// Models via Google APIs that require explicit tool call IDs in function calls/responses.
pub fn requires_tool_call_id(model_id: &str) -> bool {
	let gemini_major_version = get_gemini_major_version(model_id);
	model_id.starts_with("claude-")
		|| model_id.starts_with("gpt-oss-")
		|| gemini_major_version.is_some_and(|version| version >= 3.0)
}

static GEMINI_MAJOR_VERSION: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^gemini(?:-live)?-([0-9]+)").expect("valid regex"));

fn get_gemini_major_version(model_id: &str) -> Option<f64> {
	let lowered = model_id.to_lowercase();
	let captures = GEMINI_MAJOR_VERSION.captures(&lowered)?;
	captures[1].parse::<f64>().ok()
}

fn supports_multimodal_function_response(model_id: &str) -> bool {
	match get_gemini_major_version(model_id) {
		Some(gemini_major_version) => gemini_major_version >= 3.0,
		None => true,
	}
}

/// `id.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64)`: the regex has no `u` flag, so an astral character (two UTF-16
/// code units) becomes two underscores.
fn sanitize_tool_call_id(id: &str) -> String {
	let mut sanitized = String::with_capacity(id.len());
	for ch in id.chars() {
		if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
			sanitized.push(ch);
		} else {
			for _ in 0..ch.len_utf16() {
				sanitized.push('_');
			}
		}
	}
	sanitized.chars().take(64).collect()
}

fn text_part(text: &str) -> Value {
	json!({ "text": sanitize_surrogates(text) })
}

fn inline_data_part(mime_type: &str, data: &str) -> Value {
	json!({ "inlineData": { "mimeType": mime_type, "data": data } })
}

/// Convert internal messages to Gemini Content[] format.
pub fn convert_messages(model: &Model, context: &TranscriptContext) -> Vec<Value> {
	// Gemini has no mid-conversation system messages; the leading prompt is sent as systemInstruction.
	let collapsed = collapse_system_messages(context);
	let conversation = without_initial_system_message(&collapsed.messages);
	let mut contents: Vec<Value> = Vec::new();
	let normalize_tool_call_id = |id: &str, model: &Model, _source: &AssistantMessage| -> String {
		if !requires_tool_call_id(&model.id) {
			return id.to_string();
		}
		sanitize_tool_call_id(id)
	};

	let transformed_messages = transform_messages(conversation, model, Some(&normalize_tool_call_id));

	for msg in &transformed_messages {
		match msg {
			Message::User(user) => match &user.content {
				UserMessageContent::Text(text) => {
					contents.push(json!({ "role": "user", "parts": [text_part(text)] }));
				}
				UserMessageContent::Blocks(blocks) => {
					let parts: Vec<Value> = blocks
						.iter()
						.map(|item| match item {
							UserContent::Text(text) => text_part(&text.text),
							UserContent::Image(image) => inline_data_part(&image.mime_type, &image.data),
						})
						.collect();
					if parts.is_empty() {
						continue;
					}
					contents.push(json!({ "role": "user", "parts": parts }));
				}
			},
			Message::Assistant(assistant) => {
				let mut parts: Vec<Value> = Vec::new();
				// Check if message is from same provider and model - only then keep thinking blocks
				let is_same_provider_and_model = assistant.provider == model.provider && assistant.model == model.id;

				for block in &assistant.content {
					match block {
						AssistantContent::Text(text) => {
							let thought_signature =
								resolve_thought_signature(is_same_provider_and_model, text.text_signature.as_deref());
							// Skip empty text blocks — unless they carry a thought signature. Gemini can attach
							// the signature to a part whose visible text is empty and requires it echoed back;
							// dropping it breaks the reasoning chain and the model intermittently ends mid-task
							// turns with a thought-only STOP (empty completion, no tool call).
							if js_trim(&text.text).is_empty() && thought_signature.is_none() {
								continue;
							}
							let mut part = Map::new();
							part.insert(
								"text".into(),
								Value::String(sanitize_surrogates(&text.text).to_string()),
							);
							if let Some(thought_signature) = thought_signature {
								part.insert("thoughtSignature".into(), Value::String(thought_signature));
							}
							parts.push(Value::Object(part));
						}
						AssistantContent::Thinking(thinking) => {
							// Only keep as thinking block if same provider AND same model
							// Otherwise convert to plain text (no tags to avoid model mimicking them)
							if is_same_provider_and_model {
								let thought_signature = resolve_thought_signature(
									is_same_provider_and_model,
									thinking.thinking_signature.as_deref(),
								);
								// Same rule as text blocks: an empty thinking block is dropped only when it
								// carries no signature (mirrors the anthropic converter's handling).
								if js_trim(&thinking.thinking).is_empty() && thought_signature.is_none() {
									continue;
								}
								let mut part = Map::new();
								part.insert("thought".into(), Value::Bool(true));
								part.insert(
									"text".into(),
									Value::String(sanitize_surrogates(&thinking.thinking).to_string()),
								);
								if let Some(thought_signature) = thought_signature {
									part.insert("thoughtSignature".into(), Value::String(thought_signature));
								}
								parts.push(Value::Object(part));
							} else {
								// Cross-provider/model: the signature is unusable, empty blocks stay dropped.
								if js_trim(&thinking.thinking).is_empty() {
									continue;
								}
								parts.push(text_part(&thinking.thinking));
							}
						}
						AssistantContent::ToolCall(tool_call) => {
							let thought_signature = resolve_thought_signature(
								is_same_provider_and_model,
								tool_call.thought_signature.as_deref(),
							);
							let mut function_call = Map::new();
							function_call.insert("name".into(), Value::String(tool_call.name.clone()));
							function_call.insert("args".into(), Value::Object(tool_call.arguments.clone()));
							if requires_tool_call_id(&model.id) {
								function_call.insert("id".into(), Value::String(tool_call.id.clone()));
							}
							let mut part = Map::new();
							part.insert("functionCall".into(), Value::Object(function_call));
							if let Some(thought_signature) = thought_signature {
								part.insert("thoughtSignature".into(), Value::String(thought_signature));
							}
							parts.push(Value::Object(part));
						}
					}
				}

				if parts.is_empty() {
					continue;
				}
				contents.push(json!({ "role": "model", "parts": parts }));
			}
			Message::ToolResult(tool_result) => {
				// Extract text and image content
				let text_result = tool_result
					.content
					.iter()
					.filter_map(|content| match content {
						UserContent::Text(text) => Some(text.text.as_str()),
						UserContent::Image(_) => None,
					})
					.collect::<Vec<_>>()
					.join("\n");
				let image_content: Vec<_> = if model.input.contains(&ModelInputModality::Image) {
					tool_result
						.content
						.iter()
						.filter_map(|content| match content {
							UserContent::Image(image) => Some(image),
							UserContent::Text(_) => None,
						})
						.collect()
				} else {
					Vec::new()
				};

				let has_text = !js_trim(&text_result).is_empty();
				let has_images = !image_content.is_empty();

				// Gemini 3+ models support multimodal function responses with images nested inside
				// functionResponse.parts. Claude and other non-Gemini models behind Cloud Code Assist /
				// Gemini < 3 still needs a separate user image turn.
				let model_supports_multimodal_function_response = supports_multimodal_function_response(&model.id);

				// Use "output" key for success, "error" key for errors as per SDK documentation
				let response_value = if has_text {
					sanitize_surrogates(&text_result).to_string()
				} else if has_images {
					"(see attached image)".to_string()
				} else {
					String::new()
				};

				let image_parts: Vec<Value> = image_content
					.iter()
					.map(|image_block| inline_data_part(&image_block.mime_type, &image_block.data))
					.collect();

				let include_id = requires_tool_call_id(&model.id);
				let mut function_response = Map::new();
				function_response.insert("name".into(), Value::String(tool_result.tool_name.clone()));
				function_response.insert(
					"response".into(),
					if tool_result.is_error {
						json!({ "error": response_value })
					} else {
						json!({ "output": response_value })
					},
				);
				if has_images && model_supports_multimodal_function_response {
					function_response.insert("parts".into(), Value::Array(image_parts.clone()));
				}
				if include_id {
					function_response.insert("id".into(), Value::String(tool_result.tool_call_id.clone()));
				}
				let function_response_part = json!({ "functionResponse": function_response });

				// Cloud Code Assist API requires all function responses to be in a single user turn.
				// Check if the last content is already a user turn with function responses and merge.
				let merge_into = contents.last_mut().filter(|last_content| {
					last_content.get("role").and_then(Value::as_str) == Some("user")
						&& last_content
							.get("parts")
							.and_then(Value::as_array)
							.is_some_and(|parts| parts.iter().any(|part| part.get("functionResponse").is_some()))
				});
				match merge_into
					.and_then(|last_content| last_content.get_mut("parts"))
					.and_then(Value::as_array_mut)
				{
					Some(parts) => parts.push(function_response_part),
					None => contents.push(json!({ "role": "user", "parts": [function_response_part] })),
				}

				// For Gemini < 3, add images in a separate user message
				if has_images && !model_supports_multimodal_function_response {
					let mut parts = vec![json!({ "text": "Tool result image:" })];
					parts.extend(image_parts);
					contents.push(json!({ "role": "user", "parts": parts }));
				}
			}
			Message::System(_) => {}
		}
	}

	contents
}

const JSON_SCHEMA_META_DECLARATIONS: [&str; 8] = [
	"$schema",
	"$id",
	"$anchor",
	"$dynamicAnchor",
	"$vocabulary",
	"$comment",
	"$defs",
	"definitions", // pre-draft-2019-09 equivalent of $defs
];

/// Strip meta-declarations from a schema obj
fn sanitize_for_open_api(schema: &Value) -> Value {
	let Value::Object(schema) = schema else {
		return schema.clone();
	};

	let mut result = Map::new();
	for (key, value) in schema {
		if JSON_SCHEMA_META_DECLARATIONS.contains(&key.as_str()) {
			continue;
		}
		result.insert(key.clone(), sanitize_for_open_api(value));
	}
	Value::Object(result)
}

/// Schema keywords whose values map names to subschemas, so their keys are not schema keywords.
const JSON_SCHEMA_NAME_MAPS: [&str; 6] = [
	"properties",
	"patternProperties",
	"$defs",
	"definitions",
	"dependentSchemas",
	"dependencies",
];

/// Keywords containing a subschema or an array of subschemas, not literal data.
const JSON_SCHEMA_SUBSCHEMAS: [&str; 16] = [
	"additionalProperties",
	"unevaluatedProperties",
	"propertyNames",
	"items",
	"prefixItems",
	"additionalItems",
	"unevaluatedItems",
	"contains",
	"allOf",
	"anyOf",
	"oneOf",
	"not",
	"if",
	"then",
	"else",
	"contentSchema",
];

/// Rewrite string and number `const` values to single-value `enum`s.
/// Gemini ignores `const` in `parametersJsonSchema` but honors `enum`.
fn rewrite_const_as_enum(schema: &Value, is_name_map: bool) -> Value {
	let object = match schema {
		Value::Array(items) => {
			return Value::Array(items.iter().map(|item| rewrite_const_as_enum(item, false)).collect());
		}
		Value::Object(object) => object,
		_ => return schema.clone(),
	};

	let mut result: Map<String, Value> = object
		.iter()
		.map(|(key, value)| {
			if is_name_map || JSON_SCHEMA_SUBSCHEMAS.contains(&key.as_str()) {
				return (key.clone(), rewrite_const_as_enum(value, false));
			}
			if JSON_SCHEMA_NAME_MAPS.contains(&key.as_str()) {
				return (key.clone(), rewrite_const_as_enum(value, true));
			}
			(key.clone(), value.clone())
		})
		.collect();
	if !is_name_map
		&& !result.contains_key("enum")
		&& let Some(value @ (Value::String(_) | Value::Number(_))) = result.get("const").cloned()
	{
		result.shift_remove("const");
		result.insert("enum".into(), Value::Array(vec![value]));
	}
	Value::Object(result)
}

/// Convert tools to Gemini function declarations format.
///
/// By default uses `parametersJsonSchema` which supports full JSON Schema (including
/// anyOf, oneOf, etc.). Gemini ignores `const` there, so string and number `const`
/// values are sent as single-value `enum`s. Set `useParameters` to true to use the legacy `parameters`
/// field instead (OpenAPI 3.03 Schema). This is needed for Cloud Code Assist with Claude
/// models, where the API translates `parameters` into Anthropic's `input_schema`.
pub fn convert_tools(
	tools: &[Tool],
	use_parameters: Option<bool>,
	supports_strict_mode: Option<bool>,
) -> PiResult<Option<Vec<Value>>> {
	let use_parameters = use_parameters.unwrap_or(false);
	let supports_strict_mode = supports_strict_mode.unwrap_or(true);
	if tools.is_empty() {
		return Ok(None);
	}
	let mut function_declarations = Vec::with_capacity(tools.len());
	for tool in tools {
		let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode, None)?;
		let parameters = get_json_schema_tool_parameters(tool, strict)?;
		let mut declaration = Map::new();
		declaration.insert("name".into(), Value::String(tool.name.clone()));
		declaration.insert("description".into(), Value::String(tool.description.clone()));
		if use_parameters {
			declaration.insert("parameters".into(), sanitize_for_open_api(&parameters));
		} else {
			declaration.insert("parametersJsonSchema".into(), rewrite_const_as_enum(&parameters, false));
		}
		function_declarations.push(Value::Object(declaration));
	}
	Ok(Some(vec![json!({ "functionDeclarations": function_declarations })]))
}

/// Gemini 3+ enforces required function parameters in validated tool-calling modes.
pub fn supports_google_strict_tool_sampling(model_id: &str) -> bool {
	get_gemini_major_version(model_id).is_some_and(|major_version| major_version >= 3.0)
}

/// Map tool choice string to Gemini FunctionCallingConfigMode.
pub fn map_tool_choice(choice: &str) -> FunctionCallingConfigMode {
	match choice {
		"auto" => FunctionCallingConfigMode::Auto,
		"none" => FunctionCallingConfigMode::None,
		"any" => FunctionCallingConfigMode::Any,
		_ => FunctionCallingConfigMode::Auto,
	}
}

pub fn resolve_google_function_calling_mode(
	tools: &[Tool],
	tool_choice: Option<&str>,
	supports_strict_mode: bool,
) -> PiResult<Option<FunctionCallingConfigMode>> {
	let mut use_strict_mode = false;
	for tool in tools {
		if resolve_json_schema_strict_sampling(tool, supports_strict_mode, None)? == Some(true) {
			use_strict_mode = true;
			break;
		}
	}
	if let Some(choice @ ("none" | "any")) = tool_choice {
		return Ok(Some(map_tool_choice(choice)));
	}
	if use_strict_mode {
		return Ok(Some(FunctionCallingConfigMode::Validated));
	}
	Ok(tool_choice.filter(|choice| !choice.is_empty()).map(map_tool_choice))
}

/// Map Gemini FinishReason to our StopReason.
pub fn map_stop_reason(reason: &str) -> PiResult<StopReason> {
	match reason {
		"STOP" => Ok(StopReason::Stop),
		"MAX_TOKENS" => Ok(StopReason::Length),
		"BLOCKLIST"
		| "PROHIBITED_CONTENT"
		| "SPII"
		| "SAFETY"
		| "IMAGE_SAFETY"
		| "IMAGE_PROHIBITED_CONTENT"
		| "IMAGE_RECITATION"
		| "IMAGE_OTHER"
		| "RECITATION"
		| "FINISH_REASON_UNSPECIFIED"
		| "OTHER"
		| "LANGUAGE"
		| "MALFORMED_FUNCTION_CALL"
		| "UNEXPECTED_TOOL_CALL"
		| "TOO_MANY_TOOL_CALLS"
		| "NO_IMAGE" => Ok(StopReason::Error),
		_ => Err(pi_host::PiError::new(format!("Unhandled stop reason: {reason}"))),
	}
}

/// Map string finish reason to our StopReason (for raw API responses).
pub fn map_stop_reason_string(reason: &str) -> StopReason {
	match reason {
		"STOP" => StopReason::Stop,
		"MAX_TOKENS" => StopReason::Length,
		_ => StopReason::Error,
	}
}

/// `Pick<StreamOptions, "maxRetries" | "maxRetryDelayMs" | "signal">` for [`retry_google_request`].
#[derive(Clone, Debug, Default)]
pub struct GoogleRetryOptions {
	pub max_retries: Option<f64>,
	pub max_retry_delay_ms: Option<f64>,
	pub signal: Option<AbortSignal>,
}

/// Run a Google GenAI SDK request with the shared provider retry policy
/// (408/409/429/5xx with backoff, honoring retry-after), mirroring how the
/// Anthropic and OpenAI adapters wrap their initial request in
/// retryProviderRequest. The SDK's ApiError has a `status` property but no
/// `headers` property, and retryProviderRequest only retries errors that carry
/// both, so normalize the error by adding the missing `headers` before
/// rethrowing.
pub async fn retry_google_request<T, F, Fut>(mut request: F, options: Option<GoogleRetryOptions>) -> PiResult<T>
where
	F: FnMut() -> Fut,
	Fut: Future<Output = PiResult<T>>,
{
	let options = options.unwrap_or_default();
	retry_provider_request(
		|| {
			let attempt = request();
			async move {
				attempt.await.map_err(|mut error| {
					if error.status.is_some() {
						match &mut error.data {
							Some(Value::Object(data)) => {
								data.entry("headers").or_insert(Value::Null);
							}
							Some(_) => {}
							None => error.data = Some(json!({ "headers": null })),
						}
					}
					error
				})
			}
		},
		ProviderRetryOptions {
			max_retries: options.max_retries,
			max_retry_delay_ms: options.max_retry_delay_ms,
			signal: options.signal,
			no_retry_statuses: None,
		},
	)
	.await
}
