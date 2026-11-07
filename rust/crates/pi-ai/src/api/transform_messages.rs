//! Port of `packages/ai/src/api/transform-messages.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS normalizes `null` or missing `content` from untyped callers to `[]` before anything else (#6259, #6276). The
//! Rust message types do that when they deserialize (see `types.rs`), so messages arrive here normalized and that
//! step has nothing to do. `normalizeToolCallId` is a borrowed closure.

use std::collections::{HashMap, HashSet};

use crate::types::{
	AssistantContent, AssistantMessage, ImageContent, Message, Model, ModelInputModality, StopReason, TextContent,
	ToolCall, ToolResultMessage, UserContent, UserMessage, UserMessageContent,
};
use crate::utils::text::js_trim;

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str = "(tool image omitted: model does not support images)";
const ERROR_TOOL_IMAGE_PLACEHOLDER: &str = "(image attached after the tool results)";

/// An error tool result split into a text-only result and the images to send after the tool results.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitErrorToolResult {
	/// The result content with each run of images replaced by a note.
	pub content: Vec<TextContent>,
	/// Text naming the tool call, to send before `images`.
	pub caption: String,
	pub images: Vec<ImageContent>,
}

/// Split the images out of an error tool result, for APIs that reject images in error results
/// (Anthropic, and Claude on Bedrock). Send the returned content as the tool result and the caption and
/// images after all tool results of the turn, in the same user message. Returns None for results
/// that need no split (#10794).
pub fn split_error_tool_result_images(msg: &ToolResultMessage) -> Option<SplitErrorToolResult> {
	if !msg.is_error {
		return None;
	}
	let images: Vec<ImageContent> = msg
		.content
		.iter()
		.filter_map(|block| match block {
			UserContent::Image(image) => Some(image.clone()),
			UserContent::Text(_) => None,
		})
		.collect();
	if images.is_empty() {
		return None;
	}
	Some(SplitErrorToolResult {
		content: replace_images_with_placeholder(&msg.content, ERROR_TOOL_IMAGE_PLACEHOLDER),
		caption: format!(
			"Images from the error result of tool call {} ({}):",
			msg.tool_call_id, msg.tool_name
		),
		images,
	})
}

fn replace_images_with_placeholder(content: &[UserContent], placeholder: &str) -> Vec<TextContent> {
	let mut result = Vec::new();
	let mut previous_was_placeholder = false;

	for block in content {
		let block = match block {
			UserContent::Image(_) => {
				if !previous_was_placeholder {
					result.push(TextContent {
						text: placeholder.to_string(),
						..TextContent::default()
					});
				}
				previous_was_placeholder = true;
				continue;
			}
			UserContent::Text(block) => block,
		};

		result.push(block.clone());
		previous_was_placeholder = block.text == placeholder;
	}

	result
}

fn text_blocks(blocks: Vec<TextContent>) -> Vec<UserContent> {
	blocks.into_iter().map(UserContent::Text).collect()
}

fn downgrade_unsupported_images(messages: &[Message], model: &Model) -> Vec<Message> {
	if model.input.contains(&ModelInputModality::Image) {
		return messages.to_vec();
	}

	messages
		.iter()
		.map(|msg| match msg {
			Message::User(
				user @ UserMessage {
					content: UserMessageContent::Blocks(blocks),
					..
				},
			) => Message::User(UserMessage {
				content: UserMessageContent::Blocks(text_blocks(replace_images_with_placeholder(
					blocks,
					NON_VISION_USER_IMAGE_PLACEHOLDER,
				))),
				..user.clone()
			}),
			Message::ToolResult(tool_result) => Message::ToolResult(ToolResultMessage {
				content: text_blocks(replace_images_with_placeholder(
					&tool_result.content,
					NON_VISION_TOOL_IMAGE_PLACEHOLDER,
				)),
				..tool_result.clone()
			}),
			_ => msg.clone(),
		})
		.collect()
}

/// `normalizeToolCallId(id, model, source)`.
pub type NormalizeToolCallId = dyn Fn(&str, &Model, &AssistantMessage) -> String;

/// Normalize tool call ID for cross-provider compatibility.
/// OpenAI Responses API generates IDs that are 450+ chars with special characters like `|`.
/// Anthropic APIs require IDs matching ^[a-zA-Z0-9_-]+$ (max 64 chars).
pub fn transform_messages(
	messages: &[Message],
	model: &Model,
	normalize_tool_call_id: Option<&NormalizeToolCallId>,
) -> Vec<Message> {
	// Build a map of original tool call IDs to normalized IDs
	let mut tool_call_id_map: HashMap<String, String> = HashMap::new();
	let image_aware_messages = downgrade_unsupported_images(messages, model);

	// First pass: transform messages (unsupported image downgrade, thinking blocks, tool call ID normalization)
	let transformed: Vec<Message> = image_aware_messages
		.into_iter()
		.map(|msg| match msg {
			// System and user messages pass through unchanged
			Message::System(_) | Message::User(_) => msg,

			// Handle toolResult messages - normalize toolCallId if we have a mapping
			Message::ToolResult(mut tool_result) => {
				if let Some(normalized_id) = tool_call_id_map.get(&tool_result.tool_call_id)
					&& !normalized_id.is_empty()
					&& *normalized_id != tool_result.tool_call_id
				{
					tool_result.tool_call_id = normalized_id.clone();
				}
				Message::ToolResult(tool_result)
			}

			// Assistant messages need transformation check
			Message::Assistant(assistant_msg) => {
				let is_same_model = assistant_msg.provider == *model.provider
					&& assistant_msg.api == *model.api
					&& assistant_msg.model == model.id;

				let transformed_content = assistant_msg
					.content
					.iter()
					.filter_map(|block| match block {
						AssistantContent::Thinking(thinking) => {
							// Thinking from another model is dropped. Signatures and encrypted reasoning are
							// only valid for the model that produced them, and sending readable thinking as
							// plain text makes the new model treat it as part of the visible reply.
							if !is_same_model {
								return None;
							}
							// Keep redacted and signed blocks even if the thinking text is empty
							// (OpenAI encrypted reasoning); they are needed for replay.
							if thinking.redacted == Some(true)
								|| thinking
									.thinking_signature
									.as_deref()
									.is_some_and(|signature| !signature.is_empty())
							{
								return Some(block.clone());
							}
							if thinking.thinking.is_empty() || js_trim(&thinking.thinking).is_empty() {
								return None;
							}
							Some(block.clone())
						}

						AssistantContent::Text(text) => {
							if is_same_model {
								return Some(block.clone());
							}
							Some(AssistantContent::Text(TextContent {
								text: text.text.clone(),
								..TextContent::default()
							}))
						}

						AssistantContent::ToolCall(tool_call) => {
							let mut normalized_tool_call: ToolCall = tool_call.clone();

							if !is_same_model
								&& tool_call
									.thought_signature
									.as_deref()
									.is_some_and(|signature| !signature.is_empty())
							{
								normalized_tool_call.thought_signature = None;
							}

							if !is_same_model && let Some(normalize_tool_call_id) = normalize_tool_call_id {
								let normalized_id = normalize_tool_call_id(&tool_call.id, model, &assistant_msg);
								if normalized_id != tool_call.id {
									tool_call_id_map.insert(tool_call.id.clone(), normalized_id.clone());
									normalized_tool_call.id = normalized_id;
								}
							}

							Some(AssistantContent::ToolCall(normalized_tool_call))
						}
					})
					.collect();

				Message::Assistant(AssistantMessage {
					content: transformed_content,
					..assistant_msg
				})
			}
		})
		.collect();

	// Second pass: insert synthetic empty tool results for orphaned tool calls
	// This preserves thinking signatures and satisfies API requirements
	let mut result: Vec<Message> = Vec::new();
	let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
	let mut existing_tool_result_ids: HashSet<String> = HashSet::new();
	// System messages are transparent to tool-call accounting: one that lands between a tool
	// call and its results is held back and emitted after the results (synthetic ones
	// included), so it never causes a duplicate result for a call that is answered later.
	let mut held_system_messages: Vec<Message> = Vec::new();
	fn close_pending_tool_calls(
		result: &mut Vec<Message>,
		pending_tool_calls: &mut Vec<ToolCall>,
		existing_tool_result_ids: &mut HashSet<String>,
		held_system_messages: &mut Vec<Message>,
	) {
		if !pending_tool_calls.is_empty() {
			for tc in pending_tool_calls.iter() {
				if !existing_tool_result_ids.contains(&tc.id) {
					result.push(Message::ToolResult(ToolResultMessage {
						tool_call_id: tc.id.clone(),
						tool_name: tc.name.clone(),
						content: vec![UserContent::Text(TextContent {
							text: "No result provided".to_string(),
							..TextContent::default()
						})],
						is_error: true,
						timestamp: pi_host::now_ms(),
						..ToolResultMessage::default()
					}));
				}
			}
			pending_tool_calls.clear();
			existing_tool_result_ids.clear();
		}
		result.append(held_system_messages);
	}

	for msg in transformed {
		match msg {
			Message::Assistant(assistant_msg) => {
				// If we have pending orphaned tool calls from a previous assistant, insert synthetic results now
				close_pending_tool_calls(
					&mut result,
					&mut pending_tool_calls,
					&mut existing_tool_result_ids,
					&mut held_system_messages,
				);

				// Skip errored/aborted assistant messages entirely.
				// These are incomplete turns that shouldn't be replayed:
				// - May have partial content (reasoning without message, incomplete tool calls)
				// - Replaying them can cause API errors (e.g., OpenAI "reasoning without following item")
				// - The model should retry from the last valid state
				if matches!(assistant_msg.stop_reason, StopReason::Error | StopReason::Aborted) {
					continue;
				}

				// Track tool calls from this assistant message
				let tool_calls: Vec<ToolCall> = assistant_msg
					.content
					.iter()
					.filter_map(|block| match block {
						AssistantContent::ToolCall(tool_call) => Some(tool_call.clone()),
						_ => None,
					})
					.collect();
				if !tool_calls.is_empty() {
					pending_tool_calls = tool_calls;
					existing_tool_result_ids = HashSet::new();
				}

				result.push(Message::Assistant(assistant_msg));
			}
			Message::ToolResult(tool_result) => {
				existing_tool_result_ids.insert(tool_result.tool_call_id.clone());
				result.push(Message::ToolResult(tool_result));
			}
			Message::System(_) => {
				if !pending_tool_calls.is_empty() {
					held_system_messages.push(msg);
				} else {
					result.push(msg);
				}
			}
			Message::User(_) => {
				// A new user turn interrupts tool flow - insert synthetic results for orphaned calls
				close_pending_tool_calls(
					&mut result,
					&mut pending_tool_calls,
					&mut existing_tool_result_ids,
					&mut held_system_messages,
				);
				result.push(msg);
			}
		}
	}

	// If the conversation ends with unresolved tool calls, synthesize results now.
	close_pending_tool_calls(
		&mut result,
		&mut pending_tool_calls,
		&mut existing_tool_result_ids,
		&mut held_system_messages,
	);

	result
}
