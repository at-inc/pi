//! Exports of `utils/transcript`. Only system messages are read (as JSON text); messages of other roles, custom roles
//! included, stay the caller's JS objects, so results that keep messages (`normalizeContext`, `collapseSystemMessages`,
//! `withoutInitialSystemMessage`, ...) hold the same objects as in the TS.

use js_sys::{Array, Object, Reflect};
use pi_ai::utils::transcript::{self, TranscriptMessage, TranscriptToolSupport};
use pi_ai::{SystemMessage, Tool};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::json::{from_js_text, to_js_text};
use crate::options::throw;

/// `createInitialSystemMessage(systemPrompt, tools)`. Its `toolsAdded` is the caller's `tools` array, as in the TS.
#[wasm_bindgen(js_name = aiCreateInitialSystemMessage)]
pub fn create_initial_system_message(system_prompt: Option<String>, tools: JsValue) -> Result<JsValue, JsValue> {
	let parsed = tool_list(&tools)?;
	let Some(message) = transcript::create_initial_system_message(system_prompt.as_deref(), parsed.as_deref()) else {
		return Ok(JsValue::UNDEFINED);
	};
	let has_tools = message.tools_added.is_some();
	let message = throw(to_js_text(&message))?;
	if has_tools {
		Reflect::set(&message, &"toolsAdded".into(), &tools)?;
	}
	Ok(message)
}

/// `normalizeContext(context)`: `{ messages }` with the leading system message built from `systemPrompt` and `tools`
/// ahead of the caller's messages.
#[wasm_bindgen(js_name = aiNormalizeContext)]
pub fn normalize_context(context: JsValue) -> Result<JsValue, JsValue> {
	let system_prompt = Reflect::get(&context, &"systemPrompt".into())?.as_string();
	let tools = Reflect::get(&context, &"tools".into())?;
	let messages = Reflect::get(&context, &"messages".into())?;
	let initial = create_initial_system_message(system_prompt, tools)?;
	let messages = if initial.is_undefined() {
		messages
	} else {
		let all = Array::from(&messages);
		all.unshift(&initial);
		all.into()
	};
	messages_object(messages)
}

/// `getInitialSystemMessage(messages)`: the caller's leading system message object.
#[wasm_bindgen(js_name = aiGetInitialSystemMessage)]
pub fn get_initial_system_message(messages: JsValue) -> Result<JsValue, JsValue> {
	let entries = entries(&messages)?;
	Ok(match transcript::get_initial_system_message(&entries) {
		Some(_) => entries[0].value.clone(),
		None => JsValue::UNDEFINED,
	})
}

/// `withoutInitialSystemMessage(messages)`: `messages` itself, or a copy without its leading system message.
#[wasm_bindgen(js_name = aiWithoutInitialSystemMessage)]
pub fn without_initial_system_message(messages: JsValue) -> Result<JsValue, JsValue> {
	let entries = entries(&messages)?;
	if transcript::get_initial_system_message(&entries).is_none() {
		return Ok(messages);
	}
	Ok(messages.unchecked_ref::<Array>().slice(1, entries.len() as u32).into())
}

/// `getCurrentTools(messages)`.
#[wasm_bindgen(js_name = aiGetCurrentTools)]
pub fn get_current_tools(messages: JsValue) -> Result<JsValue, JsValue> {
	throw(to_js_text(&transcript::get_current_tools(&entries(&messages)?)))
}

/// `getCurrentSystemMessage(messages)`.
#[wasm_bindgen(js_name = aiGetCurrentSystemMessage)]
pub fn get_current_system_message(messages: JsValue) -> Result<JsValue, JsValue> {
	match transcript::get_current_system_message(&entries(&messages)?) {
		Some(message) => throw(to_js_text(&message)),
		None => Ok(JsValue::UNDEFINED),
	}
}

/// `getCurrentSystemPrompt(messages)`.
#[wasm_bindgen(js_name = aiGetCurrentSystemPrompt)]
pub fn get_current_system_prompt(messages: JsValue) -> Result<String, JsValue> {
	Ok(transcript::get_current_system_prompt(&entries(&messages)?))
}

/// `collapseSystemMessages(context)`: the replayed system message, then the caller's other messages.
#[wasm_bindgen(js_name = aiCollapseSystemMessages)]
pub fn collapse_system_messages(context: JsValue) -> Result<JsValue, JsValue> {
	let entries = entries(&Reflect::get(&context, &"messages".into())?)?;
	let messages = Array::new();
	if let Some(head) = transcript::get_current_system_message(&entries) {
		messages.push(&throw(to_js_text(&head))?);
	}
	for entry in entries.iter().filter(|entry| entry.system.is_none()) {
		messages.push(&entry.value);
	}
	messages_object(messages.into())
}

/// `resolveTranscript(context, supportsMidConvoSystemMessages)`: `context` itself when the model accepts later system
/// messages, otherwise `collapseSystemMessages(context)`.
#[wasm_bindgen(js_name = aiResolveTranscript)]
pub fn resolve_transcript(context: JsValue, supports_mid_convo_system_messages: JsValue) -> Result<JsValue, JsValue> {
	if supports_mid_convo_system_messages.is_truthy() {
		return Ok(context);
	}
	collapse_system_messages(context)
}

/// `toToolDeclaration(tool)`.
#[wasm_bindgen(js_name = aiToToolDeclaration)]
pub fn to_tool_declaration(tool: JsValue) -> Result<JsValue, JsValue> {
	let tool: Tool = throw(from_js_text(&tool))?;
	throw(to_js_text(&transcript::to_tool_declaration(&tool)))
}

/// `declarationsEqual(left, right)`.
#[wasm_bindgen(js_name = aiDeclarationsEqual)]
pub fn declarations_equal(left: JsValue, right: JsValue) -> Result<bool, JsValue> {
	let left: Tool = throw(from_js_text(&left))?;
	let right: Tool = throw(from_js_text(&right))?;
	Ok(transcript::declarations_equal(&left, &right))
}

/// `getToolStateChanges(previous, current)`.
#[wasm_bindgen(js_name = aiGetToolStateChanges)]
pub fn get_tool_state_changes(previous: JsValue, current: JsValue) -> Result<JsValue, JsValue> {
	let previous: Vec<Tool> = throw(from_js_text(&previous))?;
	let current: Vec<Tool> = throw(from_js_text(&current))?;
	let changes = transcript::get_tool_state_changes(&previous, &current);
	let result = Object::new();
	Reflect::set(&result, &"toolsAdded".into(), &throw(to_js_text(&changes.tools_added))?)?;
	Reflect::set(
		&result,
		&"toolsRemoved".into(),
		&throw(to_js_text(&changes.tools_removed))?,
	)?;
	Ok(result.into())
}

/// `getDeclaredTools(messages)`.
#[wasm_bindgen(js_name = aiGetDeclaredTools)]
pub fn get_declared_tools(messages: JsValue) -> Result<JsValue, JsValue> {
	throw(to_js_text(&transcript::get_declared_tools(&entries(&messages)?)))
}

/// `hasToolRedefinitions(messages)`.
#[wasm_bindgen(js_name = aiHasToolRedefinitions)]
#[allow(deprecated)]
pub fn has_tool_redefinitions(messages: JsValue) -> Result<bool, JsValue> {
	Ok(transcript::has_tool_redefinitions(&entries(&messages)?))
}

/// `hasNonAdditiveToolChanges(messages)`.
#[wasm_bindgen(js_name = aiHasNonAdditiveToolChanges)]
pub fn has_non_additive_tool_changes(messages: JsValue) -> Result<bool, JsValue> {
	Ok(transcript::has_non_additive_tool_changes(&entries(&messages)?))
}

/// `resolveTranscriptTools(messages, support)`. A `support` other than `"none"` and `"changes"` takes the
/// `"additions"` path, as in the TS.
#[wasm_bindgen(js_name = aiResolveTranscriptTools)]
pub fn resolve_transcript_tools(messages: JsValue, support: JsValue) -> Result<JsValue, JsValue> {
	let support = match support.as_string().as_deref() {
		Some("none") => TranscriptToolSupport::None,
		Some("changes") => TranscriptToolSupport::Changes,
		_ => TranscriptToolSupport::Additions,
	};
	let tools = transcript::resolve_transcript_tools(&entries(&messages)?, support);
	let result = Object::new();
	Reflect::set(
		&result,
		&"requestTools".into(),
		&throw(to_js_text(&tools.request_tools))?,
	)?;
	Reflect::set(
		&result,
		&"foldedSystemMessages".into(),
		&JsValue::from(tools.folded_system_messages as f64),
	)?;
	Ok(result.into())
}

/// One message of a JS message list: the JS object, and its `SystemMessage` when its role is `"system"`.
struct Entry {
	value: JsValue,
	system: Option<SystemMessage>,
}

impl TranscriptMessage for Entry {
	fn as_system_message(&self) -> Option<&SystemMessage> {
		self.system.as_ref()
	}
}

/// The entries of a JS message list; only system messages are read.
fn entries(messages: &JsValue) -> Result<Vec<Entry>, JsValue> {
	let messages = Array::from(messages);
	let mut entries = Vec::with_capacity(messages.length() as usize);
	for value in messages.iter() {
		let is_system = Reflect::get(&value, &"role".into())?.as_string().as_deref() == Some("system");
		let system = if is_system {
			Some(throw(from_js_text(&value))?)
		} else {
			None
		};
		entries.push(Entry { value, system });
	}
	Ok(entries)
}

/// A tool list argument; `undefined` and `null` read as none.
fn tool_list(tools: &JsValue) -> Result<Option<Vec<Tool>>, JsValue> {
	if tools.is_undefined() || tools.is_null() {
		return Ok(None);
	}
	throw(from_js_text(tools)).map(Some)
}

/// `{ messages }`.
fn messages_object(messages: JsValue) -> Result<JsValue, JsValue> {
	let result = Object::new();
	Reflect::set(&result, &"messages".into(), &messages)?;
	Ok(result.into())
}
