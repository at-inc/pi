//! Port of `packages/ai/src/api/openai-responses-shared.ts`.
//! PORT-STATUS: ported
//!
//! The SDK's request and stream types are plain JSON: input items and tools are `serde_json` values built in the TS
//! object-literal key order, and stream events are read with JS semantics (truthiness, `String(value)`). A missing
//! event or item field reads as `undefined` where the TS would throw a `TypeError` on it. Content blocks are
//! addressed by their index in the shared output message instead of by object reference. Output slots are keyed by
//! the event's raw `output_index`, so events without one share a slot, as the TS `Map` keyed by `undefined` does.
//! A tool call `namespace` that is not a string is dropped, and the image callbacks receive the raw `output_index`.
//! A `function_call` or `custom_tool_call` item without a string `name` gets the name `""`, as `ToolCall.name` is a
//! `String`; the TS keeps `undefined` and so drops `name` when the call is replayed.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use futures::{Stream, StreamExt};
use indexmap::{IndexMap, IndexSet};
use pi_host::js::{json_stringify, math_max, number_to_string, utf16_len};
use pi_host::{PiError, PiResult};
use serde_json::{Map, Value, json};

use crate::api::constrained_sampling::{
	GrammarToolInputJsonBuffer, append_grammar_tool_input_json_delta, close_grammar_tool_input_json,
	get_grammar_tool_input, get_json_schema_tool_parameters, resolve_grammar_constrained_sampling,
	resolve_json_schema_strict_sampling,
};
use crate::api::transform_messages::transform_messages;
use crate::models::calculate_cost;
use crate::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, JsonObject, Message, Model, ModelInputModality,
	OnProviderStreamEvent, SharedAssistantMessage, StopReason, SystemMessage, TextContent, ThinkingContent, Tool,
	ToolCall, TranscriptContext, Usage, UsageCost, UserContent, UserMessageContent,
};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::hash::short_hash;
use crate::utils::json_parse::json_parse;
use crate::utils::sanitize_unicode::sanitize_surrogates;
use crate::utils::streaming_json::ToolCallArgumentsStream;
use crate::utils::text::{get_system_message_text, js_trim, render_system_message_update};
use crate::utils::transcript::{TranscriptToolSupport, resolve_transcript, resolve_transcript_tools};

// =============================================================================
// Utilities
// =============================================================================

/// JS truthiness of an optional JSON value; `None` is `undefined`.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
	match value {
		None | Some(Value::Null) => false,
		Some(Value::Bool(value)) => *value,
		Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0 && !number.is_nan()),
		Some(Value::String(text)) => !text.is_empty(),
		Some(Value::Array(_) | Value::Object(_)) => true,
	}
}

/// `String(value)` of an optional JSON value, as a template literal interpolates it; `None` is `undefined`.
pub(crate) fn js_string(value: Option<&Value>) -> String {
	match value {
		None => "undefined".into(),
		Some(Value::Null) => "null".into(),
		Some(Value::Bool(value)) => value.to_string(),
		Some(Value::Number(number)) => number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Some(Value::String(text)) => text.clone(),
		Some(Value::Array(items)) => join_values(items.iter().map(Some), ","),
		Some(Value::Object(_)) => "[object Object]".into(),
	}
}

/// `Array.prototype.join`: `null` and `undefined` elements are empty.
fn join_values<'a>(items: impl Iterator<Item = Option<&'a Value>>, separator: &str) -> String {
	items
		.map(|item| match item {
			None | Some(Value::Null) => String::new(),
			item => js_string(item),
		})
		.collect::<Vec<_>>()
		.join(separator)
}

/// `value[key]` of an optional JSON value.
fn field<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
	value.and_then(|value| value.get(key))
}

/// `value || 0` for a token count.
fn count(value: Option<&Value>) -> f64 {
	if truthy(value) {
		value.and_then(Value::as_f64).unwrap_or(0.0)
	} else {
		0.0
	}
}

fn encode_text_signature_v1(id: Option<&Value>, phase: Option<&Value>) -> String {
	let mut payload = Map::new();
	payload.insert("v".into(), json!(1));
	if let Some(id) = id {
		payload.insert("id".into(), id.clone());
	}
	if let Some(phase) = phase.filter(|phase| truthy(Some(phase))) {
		payload.insert("phase".into(), phase.clone());
	}
	json_stringify(&payload)
}

struct ParsedTextSignature {
	id: String,
	phase: Option<String>,
}

fn parse_text_signature(signature: Option<&str>) -> Option<ParsedTextSignature> {
	let signature = signature.filter(|signature| !signature.is_empty())?;
	if signature.starts_with('{')
		&& let Ok(parsed) = json_parse(signature)
		&& parsed.get("v").and_then(Value::as_f64) == Some(1.0)
		&& let Some(id) = parsed.get("id").and_then(Value::as_str)
	{
		let phase = parsed
			.get("phase")
			.and_then(Value::as_str)
			.filter(|phase| *phase == "commentary" || *phase == "final_answer");
		return Some(ParsedTextSignature {
			id: id.to_string(),
			phase: phase.map(str::to_string),
		});
	}
	Some(ParsedTextSignature {
		id: signature.to_string(),
		phase: None,
	})
}

fn convert_tool_result_output(model: &Model, content: &[UserContent]) -> Value {
	let text_result = content
		.iter()
		.filter_map(|block| match block {
			UserContent::Text(text) => Some(text.text.as_str()),
			UserContent::Image(_) => None,
		})
		.collect::<Vec<_>>()
		.join("\n");
	let images: Vec<_> = content
		.iter()
		.filter_map(|block| match block {
			UserContent::Image(image) => Some(image),
			UserContent::Text(_) => None,
		})
		.collect();
	let has_text = !js_trim(&text_result).is_empty();

	if images.is_empty() || !model.input.contains(&ModelInputModality::Image) {
		let text = if has_text {
			text_result.as_str()
		} else if !images.is_empty() {
			"(see attached image)"
		} else {
			"(no tool output)"
		};
		return Value::String(sanitize_surrogates(text).to_string());
	}

	let mut output = Vec::new();
	if has_text {
		output.push(json!({ "type": "input_text", "text": sanitize_surrogates(&text_result) }));
	}
	for image in images {
		output.push(json!({
			"type": "input_image",
			"detail": "auto",
			"image_url": format!("data:{};base64,{}", image.mime_type, image.data),
		}));
	}
	Value::Array(output)
}

/// `OpenAIResponsesStreamOptions.onImageGenerationCall`: the raw `output_index` and the `image_generation_call` item.
pub type OnImageGenerationCall = Rc<dyn Fn(&Value, &Value)>;
/// `OpenAIResponsesStreamOptions.onImageGenerationPartialImage`: the raw `output_index` and the base64 image.
pub type OnImageGenerationPartialImage = Rc<dyn Fn(&Value, &Value)>;
/// `OpenAIResponsesStreamOptions.resolveServiceTier`: (response tier, request tier) to the tier to price.
pub type ResolveServiceTier = Rc<dyn Fn(Option<&str>, Option<&str>) -> Option<String>>;
/// `OpenAIResponsesStreamOptions.applyServiceTierPricing`.
pub type ApplyServiceTierPricing = Rc<dyn Fn(&mut Usage, Option<&str>)>;

#[derive(Clone, Default)]
pub struct OpenAIResponsesStreamOptions {
	pub on_provider_stream_event: Option<OnProviderStreamEvent>,
	pub service_tier: Option<String>,
	pub grammar_tool_input_properties: Option<IndexMap<String, String>>,
	pub on_image_generation_call: Option<OnImageGenerationCall>,
	pub on_image_generation_partial_image: Option<OnImageGenerationPartialImage>,
	pub resolve_service_tier: Option<ResolveServiceTier>,
	pub apply_service_tier_pricing: Option<ApplyServiceTierPricing>,
}

#[derive(Clone, Debug, Default)]
pub struct ConvertResponsesMessagesOptions {
	pub include_system_prompt: Option<bool>,
	pub grammar_tool_input_properties: Option<IndexMap<String, String>>,
	/// Whether later system messages are sent in place; otherwise they are folded into the leading prompt.
	pub supports_mid_convo_system_messages: Option<bool>,
	pub supports_additional_tools: Option<bool>,
	pub supports_tool_search: Option<bool>,
	pub tool_options: Option<ConvertResponsesToolsOptions>,
	/// Effort for the reply being generated. When set, the thinking level is sent as
	/// configuration_update items instead of a changing reasoning.effort, which would invalidate the
	/// prompt cache. Each item sets the effort from that point on: one goes before every earlier
	/// reply with the level recorded on it (`providerThinkingLevel`), and one with this effort goes
	/// at the end. Earlier input therefore never changes. OpenAI documents the item before user
	/// messages only. After tool results it was verified on gpt-6-luna (October 2026) to be accepted
	/// and to apply to the next reply.
	pub configuration_update_effort: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ConvertResponsesToolsOptions {
	/// `strict`: `None` is `undefined`, `Some(None)` is `null`.
	pub strict: Option<Option<bool>>,
	pub supports_strict_mode: Option<bool>,
	pub supports_openai_grammar_tools: Option<bool>,
	pub tool_search_result: Option<bool>,
}

// =============================================================================
// Message conversion
// =============================================================================

/// How a Responses transport expresses tool changes.
///
/// `additional_tools` items accumulate: a later item adds tools and replaces same-name
/// definitions from earlier items, but it cannot replace a top-level tool and cannot remove any
/// tool (verified with strict schemas on the OpenAI, Codex, and Copilot backends, October 2026).
/// Like Codex, every tool is therefore declared in `additional_tools` items, starting with one
/// at the head of the input, and removals are announced as text. Synthetic tool search can
/// only add tools. Without mid-conversation system messages there is nothing to anchor.
pub fn get_responses_tool_support(
	supports_mid_convo_system_messages: Option<bool>,
	supports_additional_tools: Option<bool>,
	supports_tool_search: Option<bool>,
) -> TranscriptToolSupport {
	if supports_mid_convo_system_messages == Some(true) && supports_additional_tools == Some(true) {
		return TranscriptToolSupport::Changes;
	}
	if supports_tool_search == Some(true) {
		TranscriptToolSupport::Additions
	} else {
		TranscriptToolSupport::None
	}
}

/// Precedes every additional_tools item after the first. Without it, models read the latest item as
/// the complete tool set once a removal was announced; Codex precedes its incremental tool batches
/// with the same guidance. Restating the tools that are still unavailable keeps smaller models
/// (gpt-5.6-luna, gpt-6-luna) from calling them again after later loads (verified October 2026).
fn render_incremental_tools_notice(unavailable: &IndexSet<String>) -> String {
	let notice = "This is an incremental tools update. Previously declared tools remain available unless explicitly marked unavailable. A tool declared again here is available again, and its latest definition replaces the earlier one.";
	if unavailable.is_empty() {
		notice.to_string()
	} else {
		format!(
			"{notice}\nStill unavailable: {}.",
			unavailable.iter().map(String::as_str).collect::<Vec<_>>().join(", ")
		)
	}
}

fn render_tool_removal_notice(names: &[&str]) -> String {
	format!(
		"The following tools are no longer available. Do not call them:\n{}",
		names
			.iter()
			.map(|name| format!("- {name}"))
			.collect::<Vec<_>>()
			.join("\n")
	)
}

/// `part.replace(/[^a-zA-Z0-9_-]/g, "_")`, cut to 64 characters, without trailing underscores. The regex replaces
/// every UTF-16 code unit, so a character outside the BMP becomes two underscores.
fn normalize_id_part(part: &str) -> String {
	let mut sanitized = String::with_capacity(part.len());
	for ch in part.chars() {
		if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
			sanitized.push(ch);
		} else {
			for _ in 0..ch.len_utf16() {
				sanitized.push('_');
			}
		}
	}
	sanitized.truncate(64);
	sanitized.trim_end_matches('_').to_string()
}

fn build_foreign_responses_item_id(item_id: &str) -> String {
	let mut normalized = format!("fc_{}", short_hash(item_id));
	normalized.truncate(64);
	normalized
}

/// Tool state carried across the system messages of one conversion.
struct ToolChangeState {
	declared_additional_tools: bool,
	/// Tools announced as removed and not declared again since.
	unavailable_tools: IndexSet<String>,
}

pub fn convert_responses_messages(
	model: &Model,
	context: &TranscriptContext,
	allowed_tool_call_providers: &[&str],
	options: Option<&ConvertResponsesMessagesOptions>,
) -> PiResult<Vec<Value>> {
	let default_options = ConvertResponsesMessagesOptions::default();
	let options = options.unwrap_or(&default_options);
	let normalized_context = resolve_transcript(context, options.supports_mid_convo_system_messages);
	let mut messages: Vec<Value> = Vec::new();

	let allowed_provider = allowed_tool_call_providers.contains(&&*model.provider);
	let (provider, api) = (model.provider.clone(), model.api.clone());
	let normalize_tool_call_id = move |id: &str, _target_model: &Model, source: &AssistantMessage| -> String {
		if !allowed_provider {
			return normalize_id_part(id);
		}
		if !id.contains('|') {
			return normalize_id_part(id);
		}
		let mut parts = id.split('|');
		let call_id = parts.next().unwrap_or_default();
		let item_id = parts.next().unwrap_or_default();
		let normalized_call_id = normalize_id_part(call_id);
		let is_foreign_tool_call = source.provider != provider || source.api != api;
		let mut normalized_item_id = if is_foreign_tool_call {
			build_foreign_responses_item_id(item_id)
		} else {
			normalize_id_part(item_id)
		};
		// OpenAI Responses API requires item id to start with "fc"
		if !normalized_item_id.starts_with("fc_") {
			normalized_item_id = normalize_id_part(&format!("fc_{normalized_item_id}"));
		}
		format!("{normalized_call_id}|{normalized_item_id}")
	};

	let transformed_messages = transform_messages(&normalized_context.messages, model, Some(&normalize_tool_call_id));
	let tool_support = get_responses_tool_support(
		options.supports_mid_convo_system_messages,
		options.supports_additional_tools,
		options.supports_tool_search,
	);
	let transcript_tools = resolve_transcript_tools(&normalized_context.messages, tool_support);
	let mut tool_state = ToolChangeState {
		declared_additional_tools: false,
		unavailable_tools: IndexSet::new(),
	};
	let include_initial_system_message = options.include_system_prompt.unwrap_or(true);
	let supports_developer_role = model
		.compat
		.as_ref()
		.and_then(|compat| compat.get("supportsDeveloperRole"))
		!= Some(&Value::Bool(false));
	let instruction_role = if model.reasoning && supports_developer_role {
		"developer"
	} else {
		"system"
	};
	let grammar_tool_input_properties = options.grammar_tool_input_properties.as_ref();

	let mut msg_index = 0usize;
	let mut system_index = 0usize;
	for (source_index, msg) in transformed_messages.iter().enumerate() {
		let is_leading_system_message = source_index == 0 && matches!(msg, Message::System(_));
		match msg {
			Message::System(system) => {
				// transformMessages keeps system messages in order, so their position matches the transcript.
				let anchors_tool_changes = system_index >= transcript_tools.folded_system_messages;
				system_index += 1;
				if anchors_tool_changes {
					append_system_tool_additions(
						system,
						&format!("system:{msg_index}"),
						tool_support,
						instruction_role,
						options.tool_options.as_ref(),
						&mut tool_state,
						&mut messages,
					)?;
				}
				if !is_leading_system_message || include_initial_system_message {
					let mut parts = vec![if is_leading_system_message {
						get_system_message_text(system)
					} else {
						render_system_message_update(system)
					}];
					if anchors_tool_changes {
						// A redefinition is a removal plus an addition of the same name; only true removals are announced.
						let added: HashSet<&str> = system
							.tools_added
							.iter()
							.flatten()
							.map(|tool| tool.name.as_str())
							.collect();
						let removed: Vec<&str> = system
							.tools_removed
							.iter()
							.flatten()
							.map(|tool| tool.name.as_str())
							.filter(|name| !added.contains(name))
							.collect();
						if !removed.is_empty() {
							parts.push(render_tool_removal_notice(&removed));
						}
						// Recorded after this message's additions, whose notice precedes this removal notice.
						for name in removed {
							tool_state.unavailable_tools.insert(name.to_string());
						}
					}
					let text = parts
						.into_iter()
						.filter(|part| !part.is_empty())
						.collect::<Vec<_>>()
						.join("\n\n");
					if !text.is_empty() {
						messages.push(json!({ "role": instruction_role, "content": sanitize_surrogates(&text) }));
					}
				}
			}
			Message::User(user) => match &user.content {
				UserMessageContent::Text(text) => {
					messages.push(json!({
						"role": "user",
						"content": [{ "type": "input_text", "text": sanitize_surrogates(text) }],
					}));
				}
				UserMessageContent::Blocks(blocks) => {
					let content: Vec<Value> = blocks
						.iter()
						.map(|item| match item {
							UserContent::Text(text) => {
								json!({ "type": "input_text", "text": sanitize_surrogates(&text.text) })
							}
							UserContent::Image(image) => json!({
								"type": "input_image",
								"detail": "auto",
								"image_url": format!("data:{};base64,{}", image.mime_type, image.data),
							}),
						})
						.collect();
					if content.is_empty() {
						continue;
					}
					messages.push(json!({ "role": "user", "content": content }));
				}
			},
			Message::Assistant(assistant) => {
				let mut output: Vec<Value> = Vec::new();
				let is_same_provider_and_api = assistant.provider == model.provider && assistant.api == model.api;
				let is_same_model = is_same_provider_and_api && assistant.model == model.id;
				let is_different_model = is_same_provider_and_api && assistant.model != model.id;
				let mut text_block_index = 0usize;

				for block in &assistant.content {
					match block {
						AssistantContent::Thinking(thinking) => {
							if let Some(signature) =
								thinking.thinking_signature.as_deref().filter(|sig| !sig.is_empty())
							{
								output.push(json_parse(signature)?);
							}
						}
						AssistantContent::Text(text_block) => {
							let parsed_signature = parse_text_signature(text_block.text_signature.as_deref());
							let fallback_message_id = if text_block_index == 0 {
								format!("msg_pi_{msg_index}")
							} else {
								format!("msg_pi_{msg_index}_{text_block_index}")
							};
							text_block_index += 1;
							// OpenAI requires id to be max 64 characters
							let msg_id = match parsed_signature.as_ref().map(|parsed| parsed.id.as_str()) {
								None | Some("") => fallback_message_id,
								Some(id) if utf16_len(id) > 64 => format!("msg_{}", short_hash(id)),
								Some(id) => id.to_string(),
							};
							let mut item = Map::new();
							item.insert("type".into(), json!("message"));
							item.insert("role".into(), json!("assistant"));
							item.insert(
								"content".into(),
								json!([{ "type": "output_text", "text": sanitize_surrogates(&text_block.text), "annotations": [] }]),
							);
							item.insert("status".into(), json!("completed"));
							item.insert("id".into(), json!(msg_id));
							if let Some(phase) = parsed_signature.and_then(|parsed| parsed.phase) {
								item.insert("phase".into(), json!(phase));
							}
							output.push(Value::Object(item));
						}
						AssistantContent::ToolCall(tool_call) => {
							let mut id_parts = tool_call.id.split('|');
							let call_id = id_parts.next().unwrap_or_default();
							let item_id_raw = id_parts.next();
							let custom_input_property =
								grammar_tool_input_properties.and_then(|properties| properties.get(&tool_call.name));

							// For different-model messages, set id to undefined to avoid pairing validation.
							// OpenAI tracks which item IDs were paired with rs_xxx reasoning items.
							// By omitting the id, we avoid triggering that validation (like cross-provider does).
							// Also drop ids that do not match the replayed item type: function_call ids must be fc_*
							// and custom_tool_call ids must be ctc_*. Foreign tool call ids are normalized to fc_*, and
							// a call can switch between the two types when grammar tool support differs.
							let item_id_prefix = if custom_input_property.is_none() { "fc_" } else { "ctc_" };
							let item_id = item_id_raw
								.filter(|item_id| !is_different_model && item_id.starts_with(item_id_prefix));

							let mut item = Map::new();
							match custom_input_property {
								Some(property) => {
									item.insert("type".into(), json!("custom_tool_call"));
									if let Some(item_id) = item_id {
										item.insert("id".into(), json!(item_id));
									}
									item.insert("call_id".into(), json!(call_id));
									item.insert("name".into(), json!(tool_call.name));
									let input =
										get_grammar_tool_input(&tool_call.name, &tool_call.arguments, property)?;
									item.insert("input".into(), json!(sanitize_surrogates(&input)));
								}
								None => {
									item.insert("type".into(), json!("function_call"));
									if let Some(item_id) = item_id {
										item.insert("id".into(), json!(item_id));
									}
									item.insert("call_id".into(), json!(call_id));
									item.insert("name".into(), json!(tool_call.name));
									item.insert("arguments".into(), json!(json_stringify(&tool_call.arguments)));
								}
							}
							if is_same_model && let Some(namespace) = &tool_call.namespace {
								item.insert("namespace".into(), json!(namespace));
							}
							output.push(Value::Object(item));
						}
					}
				}
				if output.is_empty() {
					continue;
				}
				if options.configuration_update_effort.is_some()
					&& is_same_model
					&& let Some(level) = &assistant.provider_thinking_level
				{
					messages.push(configuration_update(level));
				}
				messages.extend(output);
			}
			Message::ToolResult(tool_result) => {
				let call_id = tool_result.tool_call_id.split('|').next().unwrap_or_default();
				let output = convert_tool_result_output(model, &tool_result.content);
				let is_custom = grammar_tool_input_properties
					.is_some_and(|properties| properties.contains_key(&tool_result.tool_name));
				messages.push(json!({
					"type": if is_custom { "custom_tool_call_output" } else { "function_call_output" },
					"call_id": call_id,
					"output": output,
				}));
			}
		}
		if !is_leading_system_message {
			msg_index += 1;
		}
	}
	if let Some(effort) = &options.configuration_update_effort {
		messages.push(configuration_update(effort));
	}

	Ok(messages)
}

fn configuration_update(effort: &str) -> Value {
	json!({ "type": "configuration_update", "reasoning": { "effort": effort } })
}

fn append_system_tool_additions(
	message: &SystemMessage,
	seed: &str,
	tool_support: TranscriptToolSupport,
	instruction_role: &str,
	tool_options: Option<&ConvertResponsesToolsOptions>,
	state: &mut ToolChangeState,
	messages: &mut Vec<Value>,
) -> PiResult<()> {
	let tools: &[Tool] = message.tools_added.as_deref().unwrap_or_default();
	for tool in tools {
		state.unavailable_tools.shift_remove(&tool.name);
	}
	if tools.is_empty() {
		return Ok(());
	}
	if tool_support == TranscriptToolSupport::Changes {
		if state.declared_additional_tools {
			messages.push(json!({
				"role": instruction_role,
				"content": render_incremental_tools_notice(&state.unavailable_tools),
			}));
		}
		state.declared_additional_tools = true;
		messages.push(json!({
			"type": "additional_tools",
			"role": "developer",
			"tools": convert_responses_tools(tools, tool_options)?,
		}));
		return Ok(());
	}
	if tool_support != TranscriptToolSupport::Additions {
		return Ok(());
	}
	let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
	let call_id = format!("pi_tool_load_{}", short_hash(&format!("{seed}:{}", names.join(","))));
	messages.push(json!({
		"type": "tool_search_call",
		"call_id": call_id,
		"execution": "client",
		"status": "completed",
		"arguments": { "query": names.join(" "), "limit": names.len() },
	}));
	let search_options = ConvertResponsesToolsOptions {
		tool_search_result: Some(true),
		..tool_options.cloned().unwrap_or_default()
	};
	messages.push(json!({
		"type": "tool_search_output",
		"call_id": call_id,
		"execution": "client",
		"status": "completed",
		"tools": convert_responses_tools(tools, Some(&search_options))?,
	}));
	Ok(())
}

// =============================================================================
// Tool conversion
// =============================================================================

pub fn convert_responses_tools(tools: &[Tool], options: Option<&ConvertResponsesToolsOptions>) -> PiResult<Vec<Value>> {
	// `None` is `null`.
	let default_strict: Option<bool> = match options.and_then(|options| options.strict) {
		None => Some(false),
		Some(strict) => strict,
	};
	let supports_strict_mode = options.and_then(|options| options.supports_strict_mode).unwrap_or(true);
	let supports_openai_grammar_tools = options
		.and_then(|options| options.supports_openai_grammar_tools)
		.unwrap_or(false);
	let defer_loading = options.and_then(|options| options.tool_search_result) == Some(true);

	tools
		.iter()
		.map(|tool| {
			if let Some(grammar) = resolve_grammar_constrained_sampling(tool, supports_openai_grammar_tools)? {
				let mut custom_tool = Map::new();
				custom_tool.insert("type".into(), json!("custom"));
				custom_tool.insert("name".into(), json!(tool.name));
				custom_tool.insert("description".into(), json!(tool.description));
				custom_tool.insert(
					"format".into(),
					json!({ "type": "grammar", "syntax": grammar.format.as_str(), "definition": grammar.definition }),
				);
				if defer_loading {
					custom_tool.insert("defer_loading".into(), json!(true));
				}
				return Ok(Value::Object(custom_tool));
			}

			let constrained_strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode, None)?;
			let strict = constrained_strict.or(default_strict);
			let mut function_tool = Map::new();
			function_tool.insert("type".into(), json!("function"));
			function_tool.insert("name".into(), json!(tool.name));
			function_tool.insert("description".into(), json!(tool.description));
			function_tool.insert(
				"parameters".into(),
				get_json_schema_tool_parameters(tool, Some(strict == Some(true)))?,
			);
			if defer_loading {
				function_tool.insert("defer_loading".into(), json!(true));
			}
			if supports_strict_mode {
				function_tool.insert("strict".into(), strict.map_or(Value::Null, Value::Bool));
			}
			Ok(Value::Object(function_tool))
		})
		.collect()
}

// =============================================================================
// Stream processing
// =============================================================================

/// Stream state of a tool call, kept in its output slot and never on the content block, so it never reaches partials:
/// the JSON arguments of a function call, or the input of a custom (grammar) tool call.
enum ToolCallInput {
	Json(ToolCallArgumentsStream),
	Custom(CustomInput),
}

struct CustomInput {
	property: String,
	buffer: GrammarToolInputJsonBuffer,
	/// JSON text of the item's initial input, sent with the first delta.
	unsent: String,
}

enum ResponsesOutputSlot {
	Thinking { content_index: usize },
	Text { content_index: usize },
	ToolCall { content_index: usize, input: ToolCallInput },
}

/// The `outputSlots` key of an event: its raw `output_index`.
fn slot_key(event: &Value) -> String {
	match event.get("output_index") {
		None => "undefined".into(),
		Some(index) => json_stringify(index),
	}
}

/// Mutable state of one `processResponsesStream` run.
struct StreamState<'a> {
	output: &'a SharedAssistantMessage,
	stream: &'a AssistantMessageEventStream,
	options: Option<&'a OpenAIResponsesStreamOptions>,
	output_slots: HashMap<String, ResponsesOutputSlot>,
	/// Content indices of the tool calls whose output_item.done has not arrived yet.
	unfinished_tool_calls: IndexSet<usize>,
	reasoning_blocks_by_id: HashMap<String, usize>,
}

pub async fn process_responses_stream<S>(
	mut openai_stream: S,
	output: &SharedAssistantMessage,
	stream: &AssistantMessageEventStream,
	model: &Model,
	options: Option<&OpenAIResponsesStreamOptions>,
) -> PiResult<()>
where
	S: Stream<Item = PiResult<Value>> + Unpin,
{
	let mut saw_terminal_response_event = false;
	let mut state = StreamState {
		output,
		stream,
		options,
		output_slots: HashMap::new(),
		unfinished_tool_calls: IndexSet::new(),
		reasoning_blocks_by_id: HashMap::new(),
	};

	while let Some(event) = openai_stream.next().await {
		let event = event?;
		if let Some(on_event) = options.and_then(|options| options.on_provider_stream_event.as_ref()) {
			on_event(&event, model).await?;
		}
		let event_type = event.get("type").and_then(Value::as_str).unwrap_or_default();
		let output_index = event.get("output_index").unwrap_or(&Value::Null);
		match event_type {
			"response.created" => {
				output.borrow_mut().response_id = field(event.get("response"), "id")
					.and_then(Value::as_str)
					.map(str::to_string);
			}
			"response.output_item.added" => {
				let item = event.get("item").unwrap_or(&Value::Null);
				if item.get("type").and_then(Value::as_str) == Some("image_generation_call")
					&& let Some(callback) = options.and_then(|options| options.on_image_generation_call.as_ref())
				{
					callback(output_index, item);
				}
				state.create_slot(slot_key(&event), item)?;
			}
			"response.image_generation_call.partial_image" => {
				if let Some(callback) = options.and_then(|options| options.on_image_generation_partial_image.as_ref()) {
					callback(output_index, event.get("partial_image_b64").unwrap_or(&Value::Null));
				}
			}
			"response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
				state.append_thinking(&slot_key(&event), js_string(event.get("delta")));
			}
			"response.reasoning_summary_part.done" => {
				state.append_thinking(&slot_key(&event), "\n\n".into());
			}
			"response.output_text.delta" | "response.refusal.delta" => {
				let Some(ResponsesOutputSlot::Text { content_index }) = state.output_slots.get(&slot_key(&event))
				else {
					continue;
				};
				let content_index = *content_index;
				let delta = js_string(event.get("delta"));
				if let AssistantContent::Text(block) = &mut output.borrow_mut().content[content_index] {
					block.text.push_str(&delta);
				}
				stream.push(AssistantMessageEvent::TextDelta {
					content_index,
					delta,
					partial: output.clone(),
				});
			}
			"response.function_call_arguments.delta" => {
				let Some(ResponsesOutputSlot::ToolCall {
					content_index,
					input: ToolCallInput::Json(arguments),
				}) = state.output_slots.get_mut(&slot_key(&event))
				else {
					continue;
				};
				let delta = event.get("delta");
				update_tool_call_arguments(output, *content_index, |block| {
					arguments.append(&js_string(delta), block)
				});
				push_tool_call_delta(
					stream,
					output,
					*content_index,
					delta.map(|delta| js_string(Some(delta))),
				);
			}
			"response.function_call_arguments.done" => {
				let Some(ResponsesOutputSlot::ToolCall {
					content_index,
					input: ToolCallInput::Json(arguments),
				}) = state.output_slots.get_mut(&slot_key(&event))
				else {
					continue;
				};
				let replaced = update_tool_call_arguments(output, *content_index, |block| {
					arguments.replace(&js_string(event.get("arguments")), block)
				});
				if let Some(delta) = replaced.filter(|delta| !delta.is_empty()) {
					push_tool_call_delta(stream, output, *content_index, Some(delta));
				}
			}
			"response.custom_tool_call_input.delta" => {
				let Some(ResponsesOutputSlot::ToolCall {
					content_index,
					input: ToolCallInput::Custom(input),
				}) = state.output_slots.get_mut(&slot_key(&event))
				else {
					continue;
				};
				let delta = append_custom_input(output, *content_index, input, &js_string(event.get("delta")))?;
				push_tool_call_delta(stream, output, *content_index, delta);
			}
			"response.custom_tool_call_input.done" => {
				let Some(ResponsesOutputSlot::ToolCall {
					content_index,
					input: ToolCallInput::Custom(input),
				}) = state.output_slots.get_mut(&slot_key(&event))
				else {
					continue;
				};
				let delta = close_custom_input(
					output,
					*content_index,
					input,
					event.get("input").and_then(Value::as_str),
				)?;
				push_tool_call_delta(stream, output, *content_index, delta);
			}
			"response.output_item.done" => {
				let item = event.get("item").unwrap_or(&Value::Null);
				state.finish_output_item(slot_key(&event), output_index, item)?;
			}
			"response.completed" | "response.incomplete" => {
				saw_terminal_response_event = true;
				state.finalize_response(event.get("response").unwrap_or(&Value::Null), model)?;
			}
			"error" => {
				return Err(PiError::new(format!(
					"Error Code {}: {}",
					js_string(event.get("code")),
					js_string(event.get("message"))
				)));
			}
			"response.failed" => {
				let response = event.get("response");
				output.borrow_mut().raw_stop_reason =
					field(response, "status").and_then(Value::as_str).map(str::to_string);
				let error = field(response, "error");
				let details = field(response, "incomplete_details");
				let msg = if truthy(error) {
					let code = field(error, "code");
					let message = field(error, "message");
					format!(
						"{}: {}",
						if truthy(code) {
							js_string(code)
						} else {
							"unknown".into()
						},
						if truthy(message) {
							js_string(message)
						} else {
							"no message".into()
						}
					)
				} else if truthy(field(details, "reason")) {
					format!("incomplete: {}", js_string(field(details, "reason")))
				} else {
					"Unknown error (no error details in response)".into()
				};
				return Err(PiError::new(msg));
			}
			_ => {}
		}
	}
	if !saw_terminal_response_event {
		return Err(PiError::new(
			"OpenAI Responses stream ended before a terminal response event",
		));
	}
	// The agent runs every tool call in the final message. Refuse to hand over calls whose
	// output_item.done never arrived: their arguments may be cut off or mixed up, e.g. when a
	// non-compliant server omits output_index.
	let output = output.borrow();
	if output.stop_reason == StopReason::ToolUse
		&& let Some(content_index) = state.unfinished_tool_calls.first()
		&& let AssistantContent::ToolCall(tool_call) = &output.content[*content_index]
	{
		return Err(PiError::new(format!(
			"OpenAI Responses stream completed with an unfinished tool call: {} ({})",
			tool_call.name, tool_call.id
		)));
	}
	Ok(())
}

impl StreamState<'_> {
	fn apply_message_phase_stop_reason(&self, item: &Value) {
		if item.get("type").and_then(Value::as_str) == Some("message")
			&& item.get("phase").and_then(Value::as_str) == Some("final_answer")
		{
			self.output.borrow_mut().stop_reason = StopReason::Stop;
		}
	}

	/// Appends a block and returns its content index.
	fn push_block(&self, block: AssistantContent) -> usize {
		let mut output = self.output.borrow_mut();
		output.content.push(block);
		output.content.len() - 1
	}

	fn create_slot(&mut self, key: String, item: &Value) -> PiResult<()> {
		let partial = self.output.clone();
		match item.get("type").and_then(Value::as_str) {
			Some("reasoning") => {
				let content_index = self.push_block(AssistantContent::Thinking(ThinkingContent::default()));
				self.output_slots
					.insert(key, ResponsesOutputSlot::Thinking { content_index });
				self.stream
					.push(AssistantMessageEvent::ThinkingStart { content_index, partial });
			}
			Some("message") => {
				self.apply_message_phase_stop_reason(item);
				// Keep replay identity and phase even if the stream ends before output_item.done.
				let content_index = self.push_block(AssistantContent::Text(TextContent {
					text: String::new(),
					text_signature: Some(encode_text_signature_v1(item.get("id"), item.get("phase"))),
					..TextContent::default()
				}));
				self.output_slots
					.insert(key, ResponsesOutputSlot::Text { content_index });
				self.stream
					.push(AssistantMessageEvent::TextStart { content_index, partial });
			}
			Some("function_call") => {
				let mut arguments = ToolCallArgumentsStream::new();
				let mut initial = JsonObject::new();
				if truthy(item.get("arguments")) {
					arguments.append(&js_string(item.get("arguments")), &mut initial);
				}
				let content_index = self.push_block(AssistantContent::ToolCall(ToolCall {
					id: format!("{}|{}", js_string(item.get("call_id")), js_string(item.get("id"))),
					name: item.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
					arguments: initial,
					namespace: item.get("namespace").and_then(Value::as_str).map(str::to_string),
					..ToolCall::default()
				}));
				self.unfinished_tool_calls.insert(content_index);
				self.output_slots.insert(
					key,
					ResponsesOutputSlot::ToolCall {
						content_index,
						input: ToolCallInput::Json(arguments),
					},
				);
				self.stream
					.push(AssistantMessageEvent::ToolcallStart { content_index, partial });
			}
			Some("custom_tool_call") => {
				let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
				let input_property = self
					.options
					.and_then(|options| options.grammar_tool_input_properties.as_ref())
					.and_then(|properties| properties.get(name))
					.map_or("input", String::as_str)
					.to_string();
				let input = if truthy(item.get("input")) {
					js_string(item.get("input"))
				} else {
					String::new()
				};
				let mut arguments = JsonObject::new();
				arguments.insert(input_property.clone(), json!(input));
				let content_index = self.push_block(AssistantContent::ToolCall(ToolCall {
					id: format!("{}|{}", js_string(item.get("call_id")), js_string(item.get("id"))),
					name: name.to_string(),
					arguments,
					namespace: item.get("namespace").and_then(Value::as_str).map(str::to_string),
					..ToolCall::default()
				}));
				self.unfinished_tool_calls.insert(content_index);
				let input = custom_input(input_property, &input)?;
				self.output_slots.insert(
					key,
					ResponsesOutputSlot::ToolCall {
						content_index,
						input: ToolCallInput::Custom(input),
					},
				);
				self.stream
					.push(AssistantMessageEvent::ToolcallStart { content_index, partial });
			}
			_ => {}
		}
		Ok(())
	}

	fn append_thinking(&self, key: &str, delta: String) {
		let Some(ResponsesOutputSlot::Thinking { content_index }) = self.output_slots.get(key) else {
			return;
		};
		let content_index = *content_index;
		if let AssistantContent::Thinking(block) = &mut self.output.borrow_mut().content[content_index] {
			block.thinking.push_str(&delta);
		}
		self.stream.push(AssistantMessageEvent::ThinkingDelta {
			content_index,
			delta,
			partial: self.output.clone(),
		});
	}

	fn finish_output_item(&mut self, key: String, output_index: &Value, item: &Value) -> PiResult<()> {
		let item_type = item.get("type").and_then(Value::as_str);
		if item_type == Some("image_generation_call")
			&& let Some(callback) = self
				.options
				.and_then(|options| options.on_image_generation_call.as_ref())
		{
			callback(output_index, item);
		}
		self.apply_message_phase_stop_reason(item);
		if !self.output_slots.contains_key(&key) {
			self.create_slot(key.clone(), item)?;
		}
		let output = self.output;
		let partial = output.clone();
		match (item_type, self.output_slots.get_mut(&key)) {
			(Some("reasoning"), Some(ResponsesOutputSlot::Thinking { content_index })) => {
				let content_index = *content_index;
				let summary_text = join_item_texts(item.get("summary"), |part| part.get("text"), "\n\n");
				let content_text = join_item_texts(item.get("content"), |part| part.get("text"), "\n\n");
				let thinking = {
					let mut output = output.borrow_mut();
					let AssistantContent::Thinking(block) = &mut output.content[content_index] else {
						unreachable!("a thinking slot holds a thinking block");
					};
					if !summary_text.is_empty() {
						block.thinking = summary_text;
					} else if !content_text.is_empty() {
						block.thinking = content_text;
					}
					block.thinking_signature = Some(json_stringify(item));
					block.thinking.clone()
				};
				self.reasoning_blocks_by_id
					.insert(js_string(item.get("id")), content_index);
				self.stream.push(AssistantMessageEvent::ThinkingEnd {
					content_index,
					content: thinking,
					partial,
				});
				self.output_slots.remove(&key);
			}
			(Some("message"), Some(ResponsesOutputSlot::Text { content_index })) => {
				let content_index = *content_index;
				let text = join_item_texts(
					item.get("content"),
					|part| {
						if part.get("type").and_then(Value::as_str) == Some("output_text") {
							part.get("text")
						} else {
							part.get("refusal")
						}
					},
					"",
				);
				{
					let mut output = output.borrow_mut();
					let AssistantContent::Text(block) = &mut output.content[content_index] else {
						unreachable!("a text slot holds a text block");
					};
					block.text = text.clone();
					block.text_signature = Some(encode_text_signature_v1(item.get("id"), item.get("phase")));
				}
				self.stream.push(AssistantMessageEvent::TextEnd {
					content_index,
					content: text,
					partial,
				});
				self.output_slots.remove(&key);
			}
			(
				Some("function_call"),
				Some(ResponsesOutputSlot::ToolCall {
					content_index,
					input: ToolCallInput::Json(arguments),
				}),
			) => {
				let content_index = *content_index;
				if truthy(item.get("arguments")) {
					update_tool_call_arguments(output, content_index, |block| {
						arguments.replace(&js_string(item.get("arguments")), block)
					});
				}
				let parsed = arguments.finish();
				let tool_call = {
					let mut output = output.borrow_mut();
					let AssistantContent::ToolCall(block) = &mut output.content[content_index] else {
						unreachable!("a tool call slot holds a tool call block");
					};
					block.arguments = parsed.arguments;
					block.arguments_error = parsed.arguments_error;
					if let Some(namespace) = item.get("namespace").and_then(Value::as_str) {
						block.namespace = Some(namespace.to_string());
					}
					block.clone()
				};
				self.unfinished_tool_calls.shift_remove(&content_index);
				self.stream.push(AssistantMessageEvent::ToolcallEnd {
					content_index,
					tool_call,
					partial,
				});
				self.output_slots.remove(&key);
			}
			(
				Some("custom_tool_call"),
				Some(ResponsesOutputSlot::ToolCall {
					content_index,
					input: ToolCallInput::Custom(input),
				}),
			) => {
				let content_index = *content_index;
				let delta =
					close_custom_input(output, content_index, input, item.get("input").and_then(Value::as_str))?;
				push_tool_call_delta(self.stream, output, content_index, delta);
				let tool_call = {
					let mut output = output.borrow_mut();
					let AssistantContent::ToolCall(block) = &mut output.content[content_index] else {
						unreachable!("a tool call slot holds a tool call block");
					};
					if let Some(namespace) = item.get("namespace").and_then(Value::as_str) {
						block.namespace = Some(namespace.to_string());
					}
					block.clone()
				};
				self.unfinished_tool_calls.shift_remove(&content_index);
				self.stream.push(AssistantMessageEvent::ToolcallEnd {
					content_index,
					tool_call,
					partial,
				});
				self.output_slots.remove(&key);
			}
			_ => {}
		}
		Ok(())
	}

	/// Azure OpenAI can omit reasoning.encrypted_content from response.output_item.done
	/// and provide it only in response.completed.response.output. Backfill the
	/// persisted reasoning signature from the terminal response to keep store:false
	/// multi-turn replay stateless. See https://github.com/earendil-works/pi/issues/6409.
	fn backfill_reasoning_signatures(&self, response_output: &[Value]) -> PiResult<()> {
		for item in response_output {
			if item.get("type").and_then(Value::as_str) != Some("reasoning") || !truthy(item.get("encrypted_content")) {
				continue;
			}
			let Some(content_index) = self.reasoning_blocks_by_id.get(&js_string(item.get("id"))) else {
				continue;
			};
			let mut output = self.output.borrow_mut();
			let AssistantContent::Thinking(block) = &mut output.content[*content_index] else {
				continue;
			};
			let Some(signature) = block
				.thinking_signature
				.as_deref()
				.filter(|signature| !signature.is_empty())
			else {
				continue;
			};

			let stored_item = json_parse(signature)?;
			if truthy(stored_item.get("encrypted_content")) {
				continue;
			}
			let mut updated = match stored_item {
				Value::Object(stored_item) => stored_item,
				_ => Map::new(),
			};
			updated.insert("encrypted_content".into(), item["encrypted_content"].clone());
			block.thinking_signature = Some(json_stringify(&updated));
		}
		Ok(())
	}

	fn finalize_response(&self, response: &Value, model: &Model) -> PiResult<()> {
		let response_output: &[Value] = response
			.get("output")
			.and_then(Value::as_array)
			.map_or(&[], Vec::as_slice);
		self.backfill_reasoning_signatures(response_output)?;
		// Some Responses transports omit output_item.done. Surface terminal image items as a final backfill,
		// following Senpi's native-tool reconciliation strategy:
		// https://github.com/code-yeongyu/senpi/blob/main/packages/ai/src/api/openai-responses-shared.ts
		if let Some(callback) = self
			.options
			.and_then(|options| options.on_image_generation_call.as_ref())
		{
			for (output_index, item) in response_output.iter().enumerate() {
				if item.get("type").and_then(Value::as_str) == Some("image_generation_call") {
					callback(&json!(output_index), item);
				}
			}
		}
		let mut output = self.output.borrow_mut();
		if truthy(response.get("id")) {
			output.response_id = Some(js_string(response.get("id")));
		}
		let usage = response.get("usage");
		if truthy(usage) {
			let input_details = field(usage, "input_tokens_details");
			let cached_tokens = count(field(input_details, "cached_tokens"));
			let cache_write_tokens = count(field(input_details, "cache_write_tokens"));
			output.usage = Usage {
				// OpenAI includes cached and cache-write tokens in input_tokens, so subtract both.
				input: math_max(
					0.0,
					count(field(usage, "input_tokens")) - cached_tokens - cache_write_tokens,
				),
				output: count(field(usage, "output_tokens")),
				cache_read: cached_tokens,
				cache_write: cache_write_tokens,
				reasoning: Some(count(field(field(usage, "output_tokens_details"), "reasoning_tokens"))),
				total_tokens: count(field(usage, "total_tokens")),
				cost: UsageCost::default(),
				..Usage::default()
			};
		}
		calculate_cost(model, &mut output.usage);
		if let Some(options) = self.options
			&& let Some(apply_service_tier_pricing) = &options.apply_service_tier_pricing
		{
			let response_service_tier = response.get("service_tier").and_then(Value::as_str);
			let service_tier = match &options.resolve_service_tier {
				Some(resolve_service_tier) => {
					resolve_service_tier(response_service_tier, options.service_tier.as_deref())
				}
				None => response_service_tier
					.or(options.service_tier.as_deref())
					.map(str::to_string),
			};
			apply_service_tier_pricing(&mut output.usage, service_tier.as_deref());
		}
		// Map status to stop reason. For incomplete responses, retain the provider's
		// specific reason so max-output truncation and content filtering stay distinct.
		let status = response.get("status");
		let incomplete_reason = field(response.get("incomplete_details"), "reason").and_then(Value::as_str);
		output.raw_stop_reason = match incomplete_reason.filter(|reason| !reason.is_empty()) {
			Some(reason) => Some(format!("{}.{reason}", js_string(status))),
			None => status.and_then(Value::as_str).map(str::to_string),
		};
		let (stop_reason, error_message) = map_stop_reason(status, incomplete_reason)?;
		output.stop_reason = stop_reason;
		output.error_message = error_message;
		if output.stop_reason == StopReason::Stop
			&& output
				.content
				.iter()
				.any(|block| matches!(block, AssistantContent::ToolCall(_)))
		{
			output.stop_reason = StopReason::ToolUse;
		}
		Ok(())
	}
}

/// `items?.map(pick).join(separator) || ""`.
fn join_item_texts(items: Option<&Value>, pick: impl Fn(&Value) -> Option<&Value>, separator: &str) -> String {
	match items {
		Some(Value::Array(items)) => join_values(items.iter().map(pick), separator),
		_ => String::new(),
	}
}

/// Update the arguments of the tool call block at `content_index` in place.
fn update_tool_call_arguments<R>(
	output: &SharedAssistantMessage,
	content_index: usize,
	update: impl FnOnce(&mut JsonObject) -> R,
) -> R {
	match &mut output.borrow_mut().content[content_index] {
		AssistantContent::ToolCall(block) => update(&mut block.arguments),
		_ => unreachable!("a tool call slot holds a tool call block"),
	}
}

fn push_tool_call_delta(
	stream: &AssistantMessageEventStream,
	output: &SharedAssistantMessage,
	content_index: usize,
	delta: Option<String>,
) {
	let Some(delta) = delta else {
		return;
	};
	stream.push(AssistantMessageEvent::ToolcallDelta {
		content_index,
		delta,
		partial: output.clone(),
	});
}

/// Stream state of a custom tool call that starts with `initial_input`.
fn custom_input(property: String, initial_input: &str) -> PiResult<CustomInput> {
	let mut buffer = GrammarToolInputJsonBuffer::default();
	let unsent = append_grammar_tool_input_json_delta(&mut buffer, &property, initial_input)?.unwrap_or_default();
	Ok(CustomInput {
		property,
		buffer,
		unsent,
	})
}

/// Append to a streaming custom tool input; returns the matching piece of its JSON arguments text.
fn append_custom_input(
	output: &SharedAssistantMessage,
	content_index: usize,
	input: &mut CustomInput,
	delta: &str,
) -> PiResult<Option<String>> {
	let json_delta = append_grammar_tool_input_json_delta(&mut input.buffer, &input.property, delta)?;
	Ok(send_custom_input(output, content_index, input, json_delta))
}

/// Close a custom tool input with the provider's complete input, if it sent one.
fn close_custom_input(
	output: &SharedAssistantMessage,
	content_index: usize,
	input: &mut CustomInput,
	final_input: Option<&str>,
) -> PiResult<Option<String>> {
	let json_delta = close_grammar_tool_input_json(&mut input.buffer, &input.property, final_input)?;
	Ok(send_custom_input(output, content_index, input, json_delta))
}

fn send_custom_input(
	output: &SharedAssistantMessage,
	content_index: usize,
	input: &mut CustomInput,
	json_delta: Option<String>,
) -> Option<String> {
	let mut arguments = JsonObject::new();
	arguments.insert(input.property.clone(), json!(input.buffer.input));
	update_tool_call_arguments(output, content_index, |block| *block = arguments);
	if input.unsent.is_empty() {
		return json_delta;
	}
	let sent = std::mem::take(&mut input.unsent) + json_delta.as_deref().unwrap_or_default();
	Some(sent)
}

fn map_stop_reason(status: Option<&Value>, incomplete_reason: Option<&str>) -> PiResult<(StopReason, Option<String>)> {
	if !truthy(status) {
		return Ok((StopReason::Stop, None));
	}
	match status.and_then(Value::as_str) {
		Some("completed") => Ok((StopReason::Stop, None)),
		Some("incomplete") => {
			if incomplete_reason == Some("max_output_tokens") {
				return Ok((StopReason::Length, None));
			}
			Ok((
				StopReason::Error,
				Some(match incomplete_reason.filter(|reason| !reason.is_empty()) {
					Some(reason) => format!("Response incomplete: {reason}"),
					None => "Response incomplete without a provider reason".into(),
				}),
			))
		}
		Some("failed" | "cancelled") => Ok((StopReason::Error, None)),
		// These two are wonky ...
		Some("in_progress" | "queued") => Ok((StopReason::Stop, None)),
		_ => Err(PiError::new(format!("Unhandled stop reason: {}", js_string(status)))),
	}
}
