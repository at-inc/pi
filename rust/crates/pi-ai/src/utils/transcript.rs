//! Port of `packages/ai/src/utils/transcript.ts`.
//! PORT-STATUS: reviewed
//!
//! `TranscriptMessages = readonly { role: string }[]` becomes a slice of any [`TranscriptMessage`]: the replay helpers
//! only read system messages, so agent transcripts with custom message roles implement the trait and pass through
//! unfiltered. `toToolDeclaration`'s `JSON.parse(JSON.stringify(parameters))` is a clone of the JSON value, and
//! `declarationsEqual` compares the declarations' `JSON.stringify` output.

use indexmap::{IndexMap, IndexSet};

use crate::types::{Context, Message, SystemMessage, SystemMessageContent, Tool, ToolReference};
use crate::utils::text::{content_text, get_system_message_text, js_entries};

pub use crate::types::TranscriptContext;

/// Build the leading system message for a prompt and tool set. Returns undefined when
/// both are empty, so an empty transcript stays empty.
pub fn create_initial_system_message(system_prompt: Option<&str>, tools: Option<&[Tool]>) -> Option<SystemMessage> {
	let has_system_prompt = system_prompt.is_some_and(|prompt| !prompt.is_empty());
	let has_tools = tools.is_some_and(|tools| !tools.is_empty());
	if !has_system_prompt && !has_tools {
		return None;
	}
	Some(SystemMessage {
		content: SystemMessageContent::Text(system_prompt.unwrap_or("").to_string()),
		tools_added: if has_tools { tools.map(<[Tool]>::to_vec) } else { None },
		timestamp: 0.0,
		..SystemMessage::default()
	})
}

/// Fold `Context.systemPrompt` and `Context.tools` into a leading system message.
/// This is the only entry point that produces a [`TranscriptContext`]; every
/// provider-facing function expects the result.
pub fn normalize_context(context: &Context) -> TranscriptContext {
	let initial_message = create_initial_system_message(context.system_prompt.as_deref(), context.tools.as_deref());
	let messages = match initial_message {
		Some(initial_message) => {
			let mut messages = Vec::with_capacity(context.messages.len() + 1);
			messages.push(Message::System(initial_message));
			messages.extend(context.messages.iter().cloned());
			messages
		}
		None => context.messages.clone(),
	};
	TranscriptContext::from_normalized_messages(messages)
}

/// Any message list. The replay helpers only read entries whose role is `"system"`, so
/// agent transcripts that carry custom message roles can be passed without filtering.
pub trait TranscriptMessage {
	/// The message when its role is `"system"`.
	fn as_system_message(&self) -> Option<&SystemMessage>;
}

impl TranscriptMessage for Message {
	fn as_system_message(&self) -> Option<&SystemMessage> {
		match self {
			Message::System(message) => Some(message),
			_ => None,
		}
	}
}

impl TranscriptMessage for SystemMessage {
	fn as_system_message(&self) -> Option<&SystemMessage> {
		Some(self)
	}
}

/// Return the leading system message, if the transcript starts with one.
pub fn get_initial_system_message<M: TranscriptMessage>(messages: &[M]) -> Option<&SystemMessage> {
	messages.first().and_then(TranscriptMessage::as_system_message)
}

/// Drop the leading system message for APIs that carry the prompt outside the message list.
pub fn without_initial_system_message(messages: &[Message]) -> &[Message] {
	if get_initial_system_message(messages).is_some() {
		&messages[1..]
	} else {
		messages
	}
}

/// Resolve the tools available after applying every transcript delta in order.
pub fn get_current_tools<M: TranscriptMessage>(messages: &[M]) -> Vec<Tool> {
	let mut tools: IndexMap<String, Tool> = IndexMap::new();
	for message in messages {
		let Some(message) = message.as_system_message() else {
			continue;
		};
		for tool in message.tools_removed.iter().flatten() {
			tools.shift_remove(&tool.name);
		}
		for tool in message.tools_added.iter().flatten() {
			tools.insert(tool.name.clone(), tool.clone());
		}
	}
	tools.into_values().collect()
}

/// Replay every system message into one leading system message holding the current
/// prompt and tools. Later `content` is appended to the base prompt, `sections` are
/// patched by name, and tools are resolved with [`get_current_tools`].
pub fn get_current_system_message<M: TranscriptMessage>(messages: &[M]) -> Option<SystemMessage> {
	let mut content: Vec<String> = Vec::new();
	let mut sections: IndexMap<String, String> = IndexMap::new();
	let mut timestamp: Option<f64> = None;
	for message in messages {
		let Some(message) = message.as_system_message() else {
			continue;
		};
		timestamp.get_or_insert(message.timestamp);
		let text = content_text(&message.content, "\n");
		if !text.is_empty() {
			content.push(text);
		}
		for (name, value) in message.sections.iter().flat_map(js_entries) {
			match value {
				None => {
					sections.shift_remove(name);
				}
				Some(value) => {
					sections.insert(name.clone(), value.clone());
				}
			}
		}
	}
	let tools = get_current_tools(messages);
	if timestamp.is_none() && tools.is_empty() {
		return None;
	}
	Some(SystemMessage {
		content: SystemMessageContent::Text(content.join("\n\n")),
		sections: if sections.is_empty() {
			None
		} else {
			Some(object_from_entries(sections))
		},
		tools_added: if tools.is_empty() { None } else { Some(tools) },
		timestamp: timestamp.unwrap_or(0.0),
		..SystemMessage::default()
	})
}

/// `Object.fromEntries(map)`: an object orders integer-like keys first.
fn object_from_entries(sections: IndexMap<String, String>) -> IndexMap<String, Option<String>> {
	let sections: IndexMap<String, Option<String>> =
		sections.into_iter().map(|(name, value)| (name, Some(value))).collect();
	js_entries(&sections)
		.into_iter()
		.map(|(name, value)| (name.clone(), value.clone()))
		.collect()
}

/// Render the current system prompt text after replaying every system message.
pub fn get_current_system_prompt<M: TranscriptMessage>(messages: &[M]) -> String {
	match get_current_system_message(messages) {
		Some(message) => get_system_message_text(&message),
		None => String::new(),
	}
}

/// Rebuild the transcript for APIs without mid-conversation system messages: the replayed
/// system message leads, and every later system message is dropped.
pub fn collapse_system_messages(context: &TranscriptContext) -> TranscriptContext {
	let head = get_current_system_message(&context.messages);
	let mut messages: Vec<Message> = Vec::with_capacity(context.messages.len());
	if let Some(head) = head {
		messages.push(Message::System(head));
	}
	messages.extend(
		context
			.messages
			.iter()
			.filter(|message| !matches!(message, Message::System(_)))
			.cloned(),
	);
	TranscriptContext::from_normalized_messages(messages)
}

/// Keep later system messages in place when the model accepts them; otherwise collapse them.
pub fn resolve_transcript(
	context: &TranscriptContext,
	supports_mid_convo_system_messages: Option<bool>,
) -> TranscriptContext {
	if supports_mid_convo_system_messages == Some(true) {
		context.clone()
	} else {
		collapse_system_messages(context)
	}
}

/// Strip executable and display-only fields from a tool before transcript comparison or persistence.
pub fn to_tool_declaration(tool: &Tool) -> Tool {
	Tool {
		name: tool.name.clone(),
		description: tool.description.clone(),
		parameters: tool.parameters.clone(),
		constrained_sampling: tool.constrained_sampling.clone(),
		extra: Default::default(),
	}
}

/// Whether two tools declare the same interface to the model.
///
/// Both sides go through [`to_tool_declaration`] first, which builds both objects with the same key order, so
/// comparing the serialized declarations is exact.
pub fn declarations_equal(left: &Tool, right: &Tool) -> bool {
	pi_host::js::json_stringify(&to_tool_declaration(left)) == pi_host::js::json_stringify(&to_tool_declaration(right))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolStateChanges {
	pub tools_added: Vec<Tool>,
	pub tools_removed: Vec<ToolReference>,
}

/// Compare two complete tool states. A changed definition is a removal followed by an addition.
pub fn get_tool_state_changes(previous: &[Tool], current: &[Tool]) -> ToolStateChanges {
	let previous_tools: IndexMap<&str, &Tool> = previous.iter().map(|tool| (tool.name.as_str(), tool)).collect();
	let current_tools: IndexMap<&str, &Tool> = current.iter().map(|tool| (tool.name.as_str(), tool)).collect();
	ToolStateChanges {
		tools_added: current
			.iter()
			.filter(|tool| match previous_tools.get(tool.name.as_str()) {
				None => true,
				Some(previous_tool) => !declarations_equal(previous_tool, tool),
			})
			.map(to_tool_declaration)
			.collect(),
		tools_removed: previous
			.iter()
			.filter(|tool| match current_tools.get(tool.name.as_str()) {
				None => true,
				Some(current_tool) => !declarations_equal(tool, current_tool),
			})
			.map(|tool| ToolReference {
				name: tool.name.clone(),
			})
			.collect(),
	}
}

/// Every definition referenced by transcript tool state, in first-declaration order.
pub fn get_declared_tools<M: TranscriptMessage>(messages: &[M]) -> Vec<Tool> {
	let mut definitions: IndexMap<String, Tool> = IndexMap::new();
	for message in messages {
		let Some(message) = message.as_system_message() else {
			continue;
		};
		for tool in message.tools_added.iter().flatten() {
			definitions.insert(tool.name.clone(), tool.clone());
		}
	}
	definitions.into_values().collect()
}

/// Whether a tool name was declared twice with different definitions. A transport that can
/// only reference previously declared tools by name cannot replay such a history.
#[deprecated(
	note = "No built-in transport needs this anymore: Anthropic expresses redefinitions with inline tool_definition blocks. Kept for API compatibility and will be removed in a future release."
)]
pub fn has_tool_redefinitions<M: TranscriptMessage>(messages: &[M]) -> bool {
	let mut declared: IndexMap<&str, &Tool> = IndexMap::new();
	for message in messages {
		let Some(message) = message.as_system_message() else {
			continue;
		};
		for tool in message.tools_added.iter().flatten() {
			if let Some(previous) = declared.get(tool.name.as_str())
				&& !declarations_equal(previous, tool)
			{
				return true;
			}
			declared.insert(tool.name.as_str(), tool);
		}
	}
	false
}

/// Whether tool history contains a removal or same-name redeclaration that an addition-only transport cannot replay.
pub fn has_non_additive_tool_changes<M: TranscriptMessage>(messages: &[M]) -> bool {
	let system_messages: Vec<&SystemMessage> = messages
		.iter()
		.filter_map(TranscriptMessage::as_system_message)
		.collect();
	find_last_non_additive_tool_change(&system_messages).is_some()
}

/// Index of the last system message with a removal or same-name redeclaration (TS returns -1 for none).
fn find_last_non_additive_tool_change(system_messages: &[&SystemMessage]) -> Option<usize> {
	// A name declared before counts as a redeclaration even after its removal: Kimi K3 ignores a
	// removed tool that a later tool message declares again (verified October 2026).
	let mut declared: IndexSet<&str> = IndexSet::new();
	let mut last = None;
	for (index, message) in system_messages.iter().enumerate() {
		if message
			.tools_removed
			.as_ref()
			.is_some_and(|removed| !removed.is_empty())
		{
			last = Some(index);
		}
		for tool in message.tools_added.iter().flatten() {
			if declared.contains(tool.name.as_str()) {
				last = Some(index);
			}
			declared.insert(tool.name.as_str());
		}
	}
	last
}

/// How a transport can express tool changes after the first request.
///
/// - `"none"`: only the top-level tool list exists; it always holds the current tools.
/// - `"additions"`: tools can be added at a later message, but not removed or redefined.
/// - `"changes"`: every tool, including the initial ones, is declared in place, and a later
///   declaration under the same name replaces the earlier one. Removals are not expressible on
///   the wire, so the transport announces them as text and the removed tools stay declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TranscriptToolSupport {
	#[serde(rename = "none")]
	None,
	#[serde(rename = "additions")]
	Additions,
	#[serde(rename = "changes")]
	Changes,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranscriptTools {
	/// Tools sent in the top-level request field.
	pub request_tools: Vec<Tool>,
	/// Number of leading system messages whose tool changes `requestTools` already holds. Every
	/// later system message carries its own tool changes in place.
	pub folded_system_messages: usize,
}

/// Split tool declarations between the top-level request field and in-place changes.
///
/// With `"changes"`, the top-level list stays empty and every system message, including the
/// leading one, declares its tools in place: an in-place declaration can replace an earlier
/// in-place one, but not a top-level one. With `"additions"`, a removal or redefinition cannot be
/// expressed in place, so the top-level list becomes the tool state after the last such change;
/// later additions stay anchored, and the cached prefix is invalidated once per removal instead
/// of on every later addition. With `"none"`, the top-level list is the current tool set.
pub fn resolve_transcript_tools<M: TranscriptMessage>(
	messages: &[M],
	support: TranscriptToolSupport,
) -> TranscriptTools {
	let system_messages: Vec<&SystemMessage> = messages
		.iter()
		.filter_map(TranscriptMessage::as_system_message)
		.collect();
	match support {
		TranscriptToolSupport::None => TranscriptTools {
			request_tools: get_current_tools(&system_messages),
			folded_system_messages: system_messages.len(),
		},
		TranscriptToolSupport::Changes => TranscriptTools {
			request_tools: Vec::new(),
			folded_system_messages: 0,
		},
		TranscriptToolSupport::Additions => {
			let initial = usize::from(get_initial_system_message(messages).is_some());
			let folded = initial.max(find_last_non_additive_tool_change(&system_messages).map_or(0, |index| index + 1));
			TranscriptTools {
				request_tools: get_current_tools(&system_messages[..folded.min(system_messages.len())]),
				folded_system_messages: folded,
			}
		}
	}
}

impl TranscriptMessage for &SystemMessage {
	fn as_system_message(&self) -> Option<&SystemMessage> {
		Some(self)
	}
}
