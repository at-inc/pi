//! Port of `packages/ai/src/providers/faux.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `createFauxCore()` returns a `FauxCore` struct (shared state behind an `Rc`, so clones are the same core) whose
//!   functions are methods. `FauxProviderRegistration` and `FauxProviderHandle` hold that core and deref to it for
//!   `api`, `models`, `getModel`, `state`, `setResponses`, `appendResponses` and `getPendingResponseCount`;
//!   `FauxProviderRegistration.unregister` is a callback field that compat sets, also callable as a method.
//! - `getModel()` / `getModel(id)` is one method taking `Option<&str>`, like the TS implementation signature.
//! - `FauxProviderState` is shared as `Rc<RefCell<FauxProviderState>>`; counts are `usize`.
//! - `FauxResponseFactory` always returns a future; a sync TS factory, a throwing one, and a rejecting one all map
//!   to it (`Err` is the throw). The factory gets the state `Rc`.
//! - `fauxAssistantMessage(content)` takes `impl Into<FauxAssistantContent>` for `string | FauxContentBlock |
//!   FauxContentBlock[]`; `FauxModelDefinition.cost` is a `ModelCost`, whose `tiers` pass through to the model.
//! - `fauxProvider()` returns the provider as `Rc<Provider>` so `MutableModels.setProvider` keeps its identity.
//!   Its chat API is a `ProviderStreams` over the core; `stream()` receives `ProviderStreamOptions`, so the
//!   `SimpleStreamOptions` keys the TS reads off that options object (`deferred`, `reasoning`, `toolChoice`,
//!   `thinkingBudgets`) are taken from `extra`. `deferred` follows JS truthiness: a value `DeferredOption` cannot
//!   hold becomes `Enabled(true)` when truthy (`1`, `"yes"`, `{ "window": "2h" }`) and `Enabled(false)` when falsy
//!   (`0`, `""`), so a factory on the non-deferred path sees that instead of the raw value. The other three keys are
//!   dropped when they do not deserialize (`"toolChoice": "required"`), and no other `extra` key reaches a factory
//!   or the deferred submission options, since `SimpleStreamOptions` has no `extra`: a TS factory sees
//!   `options.topLogprobs` for `{ "topLogprobs": 5 }`, a Rust one does not.
//! - `stream()` clones the context and options it stores for a deferred response; TS keeps them by reference and
//!   reads them at `fetchDeferred()`, so a caller that mutates them in between changes the TS factory input and usage
//!   estimate but not these.
//! - In the prompt text, the key order of `JSON.stringify(tool)` comes from the typed `Tool` (name, description,
//!   parameters, constrainedSampling, then app fields), and a `tool-:` entry shows only `name`, since
//!   `ToolReference` has no `extra`.
//! - `cloneMessage`'s `timestamp ?? Date.now()` and `usage ?? DEFAULT_USAGE` never apply: both fields are required
//!   in Rust.
//! - Every event's `partial` is the one live partial message (`SharedAssistantMessage`, as PORTING.md specifies). TS
//!   gives each event a shallow `{ ...partial }` copy: later deltas mutate its blocks too, but it keeps the content
//!   array of its own time, so a TS event never shows a block started after it, while a Rust event read later does.
//! - `splitStringByTokenSize` keeps the fractional index and cuts at `trunc(index)` in UTF-16 code units like `slice`.
//!   A cut that would split a surrogate pair moves one unit later, since a Rust string cannot hold a lone surrogate;
//!   only that cut moves, so `"abc😀defgh"` at token size 1 is `abc😀|def|gh` where TS gives
//!   `abc\uD83D|\uDE00def|gh`.
//! - `randomId`'s `Math.random().toString(36).slice(2)` is a random base-36 fraction rendered the same way, not
//!   V8's exact digits; trailing zeros are dropped, as V8 prints the shortest digits.
//! - `queueMicrotask` scheduling is a spawned host task, and an unpaced chunk yields to the executor once. So without
//!   `tokensPerSecond`, a consumer that aborts mid-stream stops it after a different number of deltas than in TS.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll};

use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string, utf16_len};
use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::auth::types::{ApiKeyAuth, AuthResult, ModelAuth, ProviderAuth};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::types::{
	AnyModel, Api, AssistantContent, AssistantMessage, AssistantMessageEvent, BaseModel, CacheRetention,
	CancelDeferredFunction, DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, DeferredOption,
	FetchDeferredFunction, JsonObject, Message, Model, ModelCost, ModelInputLimits, ModelInputModality, ProviderId,
	ProviderResponse, ProviderStreamOptions, ProviderStreams, SharedAssistantMessage, SimpleStreamOptions, StopReason,
	StreamOptions, TextContent, ThinkingContent, ToolCall, ToolResultMessage, TranscriptContext, Usage, UsageCost,
	UserContent, UserMessageContent,
};
use crate::utils::event_stream::{AssistantMessageEventStream, create_assistant_message_event_stream};
use crate::utils::streaming_json::ToolCallArgumentsStream;
use crate::utils::text::get_system_message_text;

const DEFAULT_API: &str = "faux";
const DEFAULT_PROVIDER: &str = "faux";
const DEFAULT_MODEL_ID: &str = "faux-1";
const DEFAULT_MODEL_NAME: &str = "Faux Model";
const DEFAULT_BASE_URL: &str = "http://localhost:0";
const DEFAULT_MIN_TOKEN_SIZE: f64 = 3.0;
const DEFAULT_MAX_TOKEN_SIZE: f64 = 5.0;

/// `DEFAULT_USAGE`.
fn default_usage() -> Usage {
	Usage {
		input: 0.0,
		output: 0.0,
		cache_read: 0.0,
		cache_write: 0.0,
		total_tokens: 0.0,
		cost: zero_cost(),
		..Usage::default()
	}
}

fn zero_cost() -> UsageCost {
	UsageCost {
		input: 0.0,
		output: 0.0,
		cache_read: 0.0,
		cache_write: 0.0,
		total: 0.0,
		..UsageCost::default()
	}
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FauxModelDefinition {
	pub id: String,
	pub name: Option<String>,
	pub reasoning: Option<bool>,
	pub input: Option<Vec<ModelInputModality>>,
	pub input_limits: Option<ModelInputLimits>,
	pub cost: Option<ModelCost>,
	pub context_window: Option<f64>,
	pub max_tokens: Option<f64>,
}

pub type FauxContentBlock = AssistantContent;

pub fn faux_text(text: &str) -> TextContent {
	TextContent {
		text: text.to_string(),
		..TextContent::default()
	}
}

pub fn faux_thinking(thinking: &str) -> ThinkingContent {
	ThinkingContent {
		thinking: thinking.to_string(),
		..ThinkingContent::default()
	}
}

/// The options of `fauxToolCall`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FauxToolCallOptions {
	pub id: Option<String>,
}

pub fn faux_tool_call(name: &str, arguments: JsonObject, options: Option<FauxToolCallOptions>) -> ToolCall {
	let options = options.unwrap_or_default();
	ToolCall {
		id: options.id.unwrap_or_else(|| random_id("tool")),
		name: name.to_string(),
		arguments,
		..ToolCall::default()
	}
}

/// The content of `fauxAssistantMessage`: `string | FauxContentBlock | FauxContentBlock[]`.
#[derive(Clone, Debug, PartialEq)]
pub enum FauxAssistantContent {
	Text(String),
	Block(FauxContentBlock),
	Blocks(Vec<FauxContentBlock>),
}

impl From<&str> for FauxAssistantContent {
	fn from(text: &str) -> Self {
		Self::Text(text.to_string())
	}
}

impl From<String> for FauxAssistantContent {
	fn from(text: String) -> Self {
		Self::Text(text)
	}
}

impl From<FauxContentBlock> for FauxAssistantContent {
	fn from(block: FauxContentBlock) -> Self {
		Self::Block(block)
	}
}

impl From<TextContent> for FauxAssistantContent {
	fn from(block: TextContent) -> Self {
		Self::Block(AssistantContent::Text(block))
	}
}

impl From<ThinkingContent> for FauxAssistantContent {
	fn from(block: ThinkingContent) -> Self {
		Self::Block(AssistantContent::Thinking(block))
	}
}

impl From<ToolCall> for FauxAssistantContent {
	fn from(block: ToolCall) -> Self {
		Self::Block(AssistantContent::ToolCall(block))
	}
}

impl From<Vec<FauxContentBlock>> for FauxAssistantContent {
	fn from(blocks: Vec<FauxContentBlock>) -> Self {
		Self::Blocks(blocks)
	}
}

fn normalize_faux_assistant_content(content: FauxAssistantContent) -> Vec<FauxContentBlock> {
	match content {
		FauxAssistantContent::Text(text) => vec![AssistantContent::Text(faux_text(&text))],
		FauxAssistantContent::Block(block) => vec![block],
		FauxAssistantContent::Blocks(blocks) => blocks,
	}
}

/// The options of `fauxAssistantMessage`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FauxAssistantMessageOptions {
	pub stop_reason: Option<StopReason>,
	pub deferred: Option<DeferredHandle>,
	pub error_message: Option<String>,
	pub response_id: Option<String>,
	pub timestamp: Option<f64>,
}

pub fn faux_assistant_message(
	content: impl Into<FauxAssistantContent>,
	options: Option<FauxAssistantMessageOptions>,
) -> AssistantMessage {
	let options = options.unwrap_or_default();
	AssistantMessage {
		content: normalize_faux_assistant_content(content.into()),
		api: Api::from(DEFAULT_API),
		provider: ProviderId::from(DEFAULT_PROVIDER),
		model: DEFAULT_MODEL_ID.to_string(),
		usage: default_usage(),
		stop_reason: options.stop_reason.unwrap_or(StopReason::Stop),
		deferred: options.deferred,
		error_message: options.error_message,
		response_id: options.response_id,
		timestamp: options.timestamp.unwrap_or_else(pi_host::now_ms),
		..AssistantMessage::default()
	}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FauxProviderState {
	pub call_count: usize,
	pub deferred_fetch_count: usize,
	pub cancelled_deferred: Vec<DeferredHandle>,
}

pub type FauxResponseFactory = Rc<
	dyn Fn(
		&TranscriptContext,
		Option<&SimpleStreamOptions>,
		&Rc<RefCell<FauxProviderState>>,
		&Model,
	) -> LocalBoxFuture<'static, PiResult<AssistantMessage>>,
>;

/// `FauxResponseStep = AssistantMessage | FauxResponseFactory`.
#[derive(Clone)]
pub enum FauxResponseStep {
	Message(AssistantMessage),
	Factory(FauxResponseFactory),
}

impl From<AssistantMessage> for FauxResponseStep {
	fn from(message: AssistantMessage) -> Self {
		Self::Message(message)
	}
}

/// `RegisterFauxProviderOptions.deferred`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FauxDeferredOptions {
	/// Number of fetches that return the original handle before the scripted response becomes ready.
	pub pending_fetches: Option<f64>,
	pub poll_after_ms: Option<f64>,
}

/// `RegisterFauxProviderOptions.tokenSize`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FauxTokenSize {
	pub min: Option<f64>,
	pub max: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RegisterFauxProviderOptions {
	pub api: Option<String>,
	pub provider: Option<String>,
	pub models: Option<Vec<FauxModelDefinition>>,
	pub deferred: Option<FauxDeferredOptions>,
	pub tokens_per_second: Option<f64>,
	pub token_size: Option<FauxTokenSize>,
}

pub struct FauxProviderRegistration {
	pub core: FauxCore,
	pub unregister: Rc<dyn Fn()>,
}

impl FauxProviderRegistration {
	/// `registration.unregister()`.
	pub fn unregister(&self) {
		(self.unregister)();
	}
}

impl Deref for FauxProviderRegistration {
	type Target = FauxCore;

	fn deref(&self) -> &Self::Target {
		&self.core
	}
}

pub struct FauxProviderHandle {
	pub provider: Rc<Provider>,
	pub core: FauxCore,
}

impl Deref for FauxProviderHandle {
	type Target = FauxCore;

	fn deref(&self) -> &Self::Target {
		&self.core
	}
}

fn estimate_tokens(text: &str) -> f64 {
	(utf16_len(text) as f64 / 4.0).ceil()
}

fn random_id(prefix: &str) -> String {
	format!(
		"{prefix}:{}:{}",
		number_to_string(pi_host::now_ms()),
		random_base36_fraction()
	)
}

/// The digits after `0.` of `Math.random().toString(36)`.
fn random_base36_fraction() -> String {
	let mut fraction = rand::random::<f64>();
	let mut digits = String::new();
	while fraction > 0.0 && digits.len() < 11 {
		fraction *= 36.0;
		let digit = fraction.floor();
		fraction -= digit;
		digits.push(char::from_digit(digit as u32, 36).expect("a base-36 digit is below 36"));
	}
	digits.truncate(digits.trim_end_matches('0').len());
	digits
}

fn content_to_text(content: &UserMessageContent) -> String {
	match content {
		UserMessageContent::Text(text) => text.clone(),
		UserMessageContent::Blocks(blocks) => blocks
			.iter()
			.map(|block| match block {
				UserContent::Text(block) => block.text.clone(),
				UserContent::Image(block) => format!("[image:{}:{}]", block.mime_type, utf16_len(&block.data)),
			})
			.collect::<Vec<_>>()
			.join("\n"),
	}
}

fn assistant_content_to_text(content: &[AssistantContent]) -> String {
	content
		.iter()
		.map(|block| match block {
			AssistantContent::Text(block) => block.text.clone(),
			AssistantContent::Thinking(block) => block.thinking.clone(),
			AssistantContent::ToolCall(block) => format!("{}:{}", block.name, json_stringify(&block.arguments)),
		})
		.collect::<Vec<_>>()
		.join("\n")
}

fn tool_result_to_text(message: &ToolResultMessage) -> String {
	std::iter::once(message.tool_name.clone())
		.chain(
			message
				.content
				.iter()
				.map(|block| content_to_text(&UserMessageContent::Blocks(vec![block.clone()]))),
		)
		.collect::<Vec<_>>()
		.join("\n")
}

fn message_to_text(message: &Message) -> String {
	match message {
		Message::System(message) => {
			let mut parts = vec![get_system_message_text(message)];
			for tool in message.tools_removed.iter().flatten() {
				parts.push(format!("tool-:{}", json_stringify(tool)));
			}
			for tool in message.tools_added.iter().flatten() {
				parts.push(format!("tool+:{}", json_stringify(tool)));
			}
			parts.retain(|part| !part.is_empty());
			parts.join("\n")
		}
		Message::User(message) => content_to_text(&message.content),
		Message::Assistant(message) => assistant_content_to_text(&message.content),
		Message::ToolResult(message) => tool_result_to_text(message),
	}
}

fn message_role(message: &Message) -> &'static str {
	match message {
		Message::System(_) => "system",
		Message::User(_) => "user",
		Message::Assistant(_) => "assistant",
		Message::ToolResult(_) => "toolResult",
	}
}

/// Length of the prompt text that joins `messages` with blank lines.
fn joined_length(messages: &[String], count: usize) -> usize {
	let mut length = if count > 0 { (count - 1) * 2 } else { 0 };
	for message in &messages[..count] {
		length += utf16_len(message);
	}
	length
}

/// Length of the common prefix of the two joined prompts. Equal messages are compared whole; characters are compared
/// only from the first message that differs.
fn common_prompt_prefix_length(previous: &[String], current: &[String]) -> usize {
	let mut index = 0;
	while index < previous.len() && index < current.len() && previous[index] == current[index] {
		index += 1;
	}
	let rest = |messages: &[String]| -> String {
		if index == messages.len() {
			return String::new();
		}
		format!(
			"{}{}",
			if index > 0 { "\n\n" } else { "" },
			messages[index..].join("\n\n")
		)
	};
	joined_length(previous, index) + common_prefix_length(&rest(previous), &rest(current))
}

fn common_prefix_length(a: &str, b: &str) -> usize {
	a.encode_utf16()
		.zip(b.encode_utf16())
		.take_while(|(a, b)| a == b)
		.count()
}

fn with_usage_estimate(
	message: AssistantMessage,
	context: &TranscriptContext,
	options: Option<&StreamOptions>,
	prompt_cache: &RefCell<HashMap<String, Vec<String>>>,
) -> AssistantMessage {
	// One text per message; the whole prompt joins them with blank lines.
	let prompt: Vec<String> = context
		.messages
		.iter()
		.map(|message| format!("{}:{}", message_role(message), message_to_text(message)))
		.collect();
	let prompt_length = joined_length(&prompt, prompt.len());
	let prompt_tokens = (prompt_length as f64 / 4.0).ceil();
	let output_tokens = estimate_tokens(&assistant_content_to_text(&message.content));
	let mut input = prompt_tokens;
	let mut cache_read = 0.0;
	let mut cache_write = 0.0;
	let session_id = options.and_then(|options| options.session_id.as_deref());

	if let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty())
		&& options.and_then(|options| options.cache_retention) != Some(CacheRetention::None)
	{
		let previous_prompt = prompt_cache.borrow().get(session_id).cloned();
		if let Some(previous_prompt) = previous_prompt {
			let cached_chars = common_prompt_prefix_length(&previous_prompt, &prompt);
			cache_read = (cached_chars as f64 / 4.0).ceil();
			cache_write = f64::max(0.0, prompt_tokens - cache_read);
		} else {
			cache_write = prompt_tokens;
		}
		// Every prompt token is either read from or written to the simulated cache.
		input = 0.0;
		prompt_cache.borrow_mut().insert(session_id.to_string(), prompt);
	}

	AssistantMessage {
		usage: Usage {
			input,
			output: output_tokens,
			cache_read,
			cache_write,
			total_tokens: input + output_tokens + cache_read + cache_write,
			cost: zero_cost(),
			..Usage::default()
		},
		..message
	}
}

fn split_string_by_token_size(text: &str, min_token_size: f64, max_token_size: f64) -> Vec<String> {
	let units: Vec<u16> = text.encode_utf16().collect();
	let mut chunks = Vec::new();
	let mut index = 0.0;
	let mut start = 0;
	while index < units.len() as f64 {
		let token_size = min_token_size + (rand::random::<f64>() * (max_token_size - min_token_size + 1.0)).floor();
		index += math_max(1.0, token_size * 4.0);
		// `slice` truncates its end (NaN is 0). Only a cut inside a surrogate pair moves, so later cuts stay at the TS
		// positions.
		let mut end = usize::min(index as usize, units.len());
		if end > 0 && end < units.len() && (0xD800..0xDC00).contains(&units[end - 1]) {
			end += 1;
		}
		chunks.push(String::from_utf16_lossy(&units[start..end]));
		start = end;
	}
	if chunks.is_empty() { vec![String::new()] } else { chunks }
}

/// `Math.min(a, b)`, which propagates NaN.
fn math_min(a: f64, b: f64) -> f64 {
	if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// `Math.max(a, b)`, which propagates NaN.
fn math_max(a: f64, b: f64) -> f64 {
	if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

fn clone_message(message: AssistantMessage, api: &str, provider: &str, model_id: &str) -> AssistantMessage {
	AssistantMessage {
		api: Api::from(api),
		provider: ProviderId::from(provider),
		model: model_id.to_string(),
		..message
	}
}

fn create_deferred_message(model: &Model, handle: &DeferredHandle) -> AssistantMessage {
	AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: default_usage(),
		stop_reason: StopReason::Deferred,
		deferred: Some(handle.clone()),
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

fn create_error_message(error: &PiError, api: &str, provider: &str, model_id: &str) -> AssistantMessage {
	AssistantMessage {
		content: Vec::new(),
		api: Api::from(api),
		provider: ProviderId::from(provider),
		model: model_id.to_string(),
		usage: default_usage(),
		stop_reason: StopReason::Error,
		error_message: Some(error.message.clone()),
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

fn create_aborted_message(partial: &AssistantMessage) -> AssistantMessage {
	AssistantMessage {
		stop_reason: StopReason::Aborted,
		error_message: Some("Request was aborted".to_string()),
		timestamp: pi_host::now_ms(),
		..partial.clone()
	}
}

async fn schedule_chunk(chunk: &str, tokens_per_second: Option<f64>) {
	match tokens_per_second {
		Some(tokens_per_second) if tokens_per_second > 0.0 => {
			let delay_ms = (estimate_tokens(chunk) / tokens_per_second) * 1000.0;
			let _ = pi_host::sleep(delay_ms, None).await;
		}
		_ => YieldNow { yielded: false }.await,
	}
}

/// The `queueMicrotask` turn: resolves on the second poll.
struct YieldNow {
	yielded: bool,
}

impl Future for YieldNow {
	type Output = ();

	fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<()> {
		if self.yielded {
			return Poll::Ready(());
		}
		self.yielded = true;
		context.waker().wake_by_ref();
		Poll::Pending
	}
}

fn shared(message: &AssistantMessage) -> Rc<RefCell<AssistantMessage>> {
	Rc::new(RefCell::new(message.clone()))
}

fn end_with_error(stream: &AssistantMessageEventStream, reason: StopReason, message: AssistantMessage) {
	stream.push(AssistantMessageEvent::Error {
		reason,
		error: shared(&message),
	});
	stream.end(Some(message));
}

async fn stream_with_deltas(
	stream: &AssistantMessageEventStream,
	message: AssistantMessage,
	min_token_size: f64,
	max_token_size: f64,
	tokens_per_second: Option<f64>,
	signal: Option<&AbortSignal>,
) -> PiResult<()> {
	let aborted = || signal.is_some_and(AbortSignal::aborted);
	let partial: SharedAssistantMessage = Rc::new(RefCell::new(AssistantMessage {
		content: Vec::new(),
		stop_reason: StopReason::Pending,
		..message.clone()
	}));
	if aborted() {
		end_with_error(stream, StopReason::Aborted, create_aborted_message(&partial.borrow()));
		return Ok(());
	}

	stream.push(AssistantMessageEvent::Start {
		partial: partial.clone(),
	});

	for (index, block) in message.content.iter().enumerate() {
		if aborted() {
			end_with_error(stream, StopReason::Aborted, create_aborted_message(&partial.borrow()));
			return Ok(());
		}

		match block {
			AssistantContent::Thinking(block) => {
				partial
					.borrow_mut()
					.content
					.push(AssistantContent::Thinking(faux_thinking("")));
				stream.push(AssistantMessageEvent::ThinkingStart {
					content_index: index,
					partial: partial.clone(),
				});
				for chunk in split_string_by_token_size(&block.thinking, min_token_size, max_token_size) {
					schedule_chunk(&chunk, tokens_per_second).await;
					if aborted() {
						end_with_error(stream, StopReason::Aborted, create_aborted_message(&partial.borrow()));
						return Ok(());
					}
					if let Some(AssistantContent::Thinking(thinking)) = partial.borrow_mut().content.get_mut(index) {
						thinking.thinking.push_str(&chunk);
					}
					stream.push(AssistantMessageEvent::ThinkingDelta {
						content_index: index,
						delta: chunk,
						partial: partial.clone(),
					});
				}
				stream.push(AssistantMessageEvent::ThinkingEnd {
					content_index: index,
					content: block.thinking.clone(),
					partial: partial.clone(),
				});
			}
			AssistantContent::Text(block) => {
				partial.borrow_mut().content.push(AssistantContent::Text(faux_text("")));
				stream.push(AssistantMessageEvent::TextStart {
					content_index: index,
					partial: partial.clone(),
				});
				for chunk in split_string_by_token_size(&block.text, min_token_size, max_token_size) {
					schedule_chunk(&chunk, tokens_per_second).await;
					if aborted() {
						end_with_error(stream, StopReason::Aborted, create_aborted_message(&partial.borrow()));
						return Ok(());
					}
					if let Some(AssistantContent::Text(text)) = partial.borrow_mut().content.get_mut(index) {
						text.text.push_str(&chunk);
					}
					stream.push(AssistantMessageEvent::TextDelta {
						content_index: index,
						delta: chunk,
						partial: partial.clone(),
					});
				}
				stream.push(AssistantMessageEvent::TextEnd {
					content_index: index,
					content: block.text.clone(),
					partial: partial.clone(),
				});
			}
			AssistantContent::ToolCall(block) => {
				partial.borrow_mut().content.push(AssistantContent::ToolCall(ToolCall {
					id: block.id.clone(),
					name: block.name.clone(),
					arguments: JsonObject::new(),
					..ToolCall::default()
				}));
				stream.push(AssistantMessageEvent::ToolcallStart {
					content_index: index,
					partial: partial.clone(),
				});
				// Partial arguments come from the same parser real providers use, so consumers see them grow.
				let mut tool_arguments = ToolCallArgumentsStream::new();
				for chunk in
					split_string_by_token_size(&json_stringify(&block.arguments), min_token_size, max_token_size)
				{
					schedule_chunk(&chunk, tokens_per_second).await;
					if aborted() {
						end_with_error(stream, StopReason::Aborted, create_aborted_message(&partial.borrow()));
						return Ok(());
					}
					if let Some(AssistantContent::ToolCall(tool_call)) = partial.borrow_mut().content.get_mut(index) {
						tool_arguments.append(&chunk, &mut tool_call.arguments);
					}
					stream.push(AssistantMessageEvent::ToolcallDelta {
						content_index: index,
						delta: chunk,
						partial: partial.clone(),
					});
				}
				if let Some(AssistantContent::ToolCall(tool_call)) = partial.borrow_mut().content.get_mut(index) {
					tool_call.arguments = block.arguments.clone();
				}
				stream.push(AssistantMessageEvent::ToolcallEnd {
					content_index: index,
					tool_call: block.clone(),
					partial: partial.clone(),
				});
			}
		}
	}

	if message.stop_reason == StopReason::Pending {
		return Err(PiError::new("Faux response ended without a stop reason"));
	}
	if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
		end_with_error(stream, message.stop_reason, message);
		return Ok(());
	}

	stream.push(AssistantMessageEvent::Done {
		reason: message.stop_reason,
		message: shared(&message),
	});
	stream.end(Some(message));
	Ok(())
}

/// A deferred submission the faux provider answers on a later fetch.
struct DeferredResponse {
	handle: DeferredHandle,
	step: FauxResponseStep,
	context: TranscriptContext,
	options: Option<SimpleStreamOptions>,
	model: Model,
	pending_fetches: f64,
	cancelled: bool,
	r#final: Option<AssistantMessage>,
}

/// The mutable state of one faux core.
struct FauxCoreInner {
	api: String,
	provider: String,
	min_token_size: f64,
	max_token_size: f64,
	pending_responses: RefCell<Vec<FauxResponseStep>>,
	tokens_per_second: Option<f64>,
	deferred: Option<FauxDeferredOptions>,
	state: Rc<RefCell<FauxProviderState>>,
	prompt_cache: RefCell<HashMap<String, Vec<String>>>,
	deferred_responses: RefCell<HashMap<String, DeferredResponse>>,
}

/// What `createFauxCore()` returns. Clones share one core.
#[derive(Clone)]
pub struct FauxCore {
	pub api: String,
	pub provider: String,
	pub models: Vec<Model>,
	pub state: Rc<RefCell<FauxProviderState>>,
	inner: Rc<FauxCoreInner>,
}

pub fn create_faux_core(options: RegisterFauxProviderOptions) -> FauxCore {
	let api = options.api.unwrap_or_else(|| random_id(DEFAULT_API));
	let provider = options.provider.unwrap_or_else(|| DEFAULT_PROVIDER.to_string());
	let token_size = options.token_size.unwrap_or_default();
	let min_token_size = math_max(
		1.0,
		math_min(
			token_size.min.unwrap_or(DEFAULT_MIN_TOKEN_SIZE),
			token_size.max.unwrap_or(DEFAULT_MAX_TOKEN_SIZE),
		),
	);
	let max_token_size = math_max(min_token_size, token_size.max.unwrap_or(DEFAULT_MAX_TOKEN_SIZE));
	let state = Rc::new(RefCell::new(FauxProviderState::default()));

	let model_definitions = match options.models {
		Some(models) if !models.is_empty() => models,
		_ => vec![FauxModelDefinition {
			id: DEFAULT_MODEL_ID.to_string(),
			name: Some(DEFAULT_MODEL_NAME.to_string()),
			reasoning: Some(false),
			input: Some(vec![ModelInputModality::Text, ModelInputModality::Image]),
			cost: Some(zero_model_cost()),
			context_window: Some(128000.0),
			max_tokens: Some(16384.0),
			..FauxModelDefinition::default()
		}],
	};
	let models = model_definitions
		.into_iter()
		.map(|definition| Model {
			base: BaseModel {
				name: definition.name.unwrap_or_else(|| definition.id.clone()),
				id: definition.id,
				api: Api::from(api.as_str()),
				provider: ProviderId::from(provider.as_str()),
				base_url: DEFAULT_BASE_URL.to_string(),
				input: definition
					.input
					.unwrap_or_else(|| vec![ModelInputModality::Text, ModelInputModality::Image]),
				input_limits: definition.input_limits,
				cost: definition.cost.unwrap_or_else(zero_model_cost),
				headers: None,
			},
			reasoning: definition.reasoning.unwrap_or(false),
			context_window: definition.context_window.unwrap_or(128000.0),
			max_tokens: definition.max_tokens.unwrap_or(16384.0),
			..Model::default()
		})
		.collect();

	FauxCore {
		api: api.clone(),
		provider: provider.clone(),
		models,
		state: state.clone(),
		inner: Rc::new(FauxCoreInner {
			api,
			provider,
			min_token_size,
			max_token_size,
			pending_responses: RefCell::new(Vec::new()),
			tokens_per_second: options.tokens_per_second,
			deferred: options.deferred,
			state,
			prompt_cache: RefCell::new(HashMap::new()),
			deferred_responses: RefCell::new(HashMap::new()),
		}),
	}
}

fn zero_model_cost() -> ModelCost {
	ModelCost {
		input: 0.0,
		output: 0.0,
		cache_read: 0.0,
		cache_write: 0.0,
		tiers: None,
		..ModelCost::default()
	}
}

fn ok_response() -> ProviderResponse {
	ProviderResponse {
		status: 200,
		headers: IndexMap::new(),
	}
}

impl FauxCoreInner {
	async fn resolve_response(
		&self,
		step: &FauxResponseStep,
		context: &TranscriptContext,
		stream_options: Option<&SimpleStreamOptions>,
		request_model: &Model,
	) -> PiResult<AssistantMessage> {
		let resolved = match step {
			FauxResponseStep::Factory(factory) => factory(context, stream_options, &self.state, request_model).await?,
			FauxResponseStep::Message(message) => message.clone(),
		};
		Ok(with_usage_estimate(
			clone_message(resolved, &self.api, &self.provider, &request_model.id),
			context,
			stream_options.map(|options| &options.base),
			&self.prompt_cache,
		))
	}

	async fn stream_with_deltas(
		&self,
		stream: &AssistantMessageEventStream,
		message: AssistantMessage,
		signal: Option<&AbortSignal>,
	) -> PiResult<()> {
		stream_with_deltas(
			stream,
			message,
			self.min_token_size,
			self.max_token_size,
			self.tokens_per_second,
			signal,
		)
		.await
	}

	async fn run_stream(
		&self,
		outer: &AssistantMessageEventStream,
		step: Option<FauxResponseStep>,
		request_model: &Model,
		context: &TranscriptContext,
		stream_options: Option<SimpleStreamOptions>,
	) -> PiResult<()> {
		if let Some(on_response) = stream_options.as_ref().and_then(|options| options.on_response.clone()) {
			on_response(ok_response(), request_model).await?;
		}
		let Some(step) = step else {
			let message = create_error_message(
				&PiError::new("No more faux responses queued"),
				&self.api,
				&self.provider,
				&request_model.id,
			);
			let message = with_usage_estimate(
				message,
				context,
				stream_options.as_ref().map(|options| &options.base),
				&self.prompt_cache,
			);
			end_with_error(outer, StopReason::Error, message);
			return Ok(());
		};

		if let Some(stream_options) = &stream_options
			&& matches!(
				stream_options.deferred,
				Some(DeferredOption::Enabled(true) | DeferredOption::Window { .. })
			) {
			let handle = DeferredHandle {
				provider: request_model.provider.to_string(),
				model_id: request_model.id.clone(),
				api: request_model.api.to_string(),
				id: random_id("deferred"),
				poll_after_ms: self.deferred.as_ref().and_then(|deferred| deferred.poll_after_ms),
				..DeferredHandle::default()
			};
			let pending_fetches = f64::max(
				0.0,
				self.deferred
					.as_ref()
					.and_then(|deferred| deferred.pending_fetches)
					.unwrap_or(0.0)
					.floor(),
			);
			self.deferred_responses.borrow_mut().insert(
				handle.id.clone(),
				DeferredResponse {
					handle: handle.clone(),
					step,
					context: context.clone(),
					options: Some(stream_options.clone()),
					model: request_model.clone(),
					pending_fetches,
					cancelled: false,
					r#final: None,
				},
			);
			return self
				.stream_with_deltas(
					outer,
					create_deferred_message(request_model, &handle),
					stream_options.signal.as_ref(),
				)
				.await;
		}

		let message = self
			.resolve_response(&step, context, stream_options.as_ref(), request_model)
			.await?;
		self.stream_with_deltas(
			outer,
			message,
			stream_options.as_ref().and_then(|options| options.signal.as_ref()),
		)
		.await
	}

	async fn run_fetch_deferred(
		&self,
		outer: &AssistantMessageEventStream,
		request_model: &Model,
		handle: &DeferredHandle,
		fetch_options: Option<DeferredFetchOptions>,
	) -> PiResult<()> {
		if let Some(on_response) = fetch_options.as_ref().and_then(|options| options.on_response.clone()) {
			on_response(ok_response(), request_model).await?;
		}
		let signal = fetch_options.as_ref().and_then(|options| options.signal.clone());
		let entry = {
			let mut deferred_responses = self.deferred_responses.borrow_mut();
			let entry = deferred_responses.get_mut(&handle.id).filter(|entry| {
				entry.handle.provider == handle.provider
					&& entry.handle.model_id == handle.model_id
					&& entry.handle.api == handle.api
			});
			let Some(entry) = entry else {
				return Err(PiError::new(format!("Unknown faux deferred response: {}", handle.id)));
			};
			if entry.cancelled {
				return Err(PiError::new(format!(
					"Faux deferred response was cancelled: {}",
					handle.id
				)));
			}
			if entry.pending_fetches > 0.0 {
				entry.pending_fetches -= 1.0;
				Err(entry.handle.clone())
			} else {
				Ok((
					entry.r#final.clone(),
					entry.step.clone(),
					entry.context.clone(),
					entry.options.clone(),
					entry.model.clone(),
				))
			}
		};
		let (stored_final, step, context, options, model) = match entry {
			Err(pending_handle) => {
				return self
					.stream_with_deltas(
						outer,
						create_deferred_message(request_model, &pending_handle),
						signal.as_ref(),
					)
					.await;
			}
			Ok(entry) => entry,
		};

		let final_message = match stored_final {
			Some(final_message) => final_message,
			None => {
				let mut submission_options = options.unwrap_or_default();
				submission_options.deferred = None;
				submission_options.signal = None;
				submission_options.on_response = None;
				let final_message = match self
					.resolve_response(&step, &context, Some(&submission_options), &model)
					.await
				{
					Ok(message) => message,
					Err(error) => create_error_message(&error, &self.api, &self.provider, &model.id),
				};
				if let Some(entry) = self.deferred_responses.borrow_mut().get_mut(&handle.id) {
					entry.r#final = Some(final_message.clone());
				}
				final_message
			}
		};
		self.stream_with_deltas(outer, final_message, signal.as_ref()).await
	}
}

impl FauxCore {
	/// `core.stream(model, context, options)`.
	pub fn stream(
		&self,
		request_model: &Model,
		context: &TranscriptContext,
		stream_options: Option<SimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		let outer = create_assistant_message_event_stream();
		let step = {
			let mut pending_responses = self.inner.pending_responses.borrow_mut();
			if pending_responses.is_empty() {
				None
			} else {
				Some(pending_responses.remove(0))
			}
		};
		self.state.borrow_mut().call_count += 1;

		let (inner, target, request_model, context) = (
			self.inner.clone(),
			outer.clone(),
			request_model.clone(),
			context.clone(),
		);
		pi_host::spawn_local(async move {
			if let Err(error) = inner
				.run_stream(&target, step, &request_model, &context, stream_options)
				.await
			{
				let message = create_error_message(&error, &inner.api, &inner.provider, &request_model.id);
				end_with_error(&target, StopReason::Error, message);
			}
		});

		outer
	}

	/// `core.streamSimple(model, context, options)`.
	pub fn stream_simple(
		&self,
		stream_model: &Model,
		context: &TranscriptContext,
		stream_options: Option<SimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		self.stream(stream_model, context, stream_options)
	}

	/// `core.fetchDeferred(model, handle, options)`.
	pub fn fetch_deferred(
		&self,
		request_model: &Model,
		handle: &DeferredHandle,
		fetch_options: Option<DeferredFetchOptions>,
	) -> AssistantMessageEventStream {
		let outer = create_assistant_message_event_stream();
		self.state.borrow_mut().deferred_fetch_count += 1;

		let (inner, target, request_model, handle) =
			(self.inner.clone(), outer.clone(), request_model.clone(), handle.clone());
		pi_host::spawn_local(async move {
			if let Err(error) = inner
				.run_fetch_deferred(&target, &request_model, &handle, fetch_options)
				.await
			{
				let message = create_error_message(&error, &inner.api, &inner.provider, &request_model.id);
				end_with_error(&target, StopReason::Error, message);
			}
		});

		outer
	}

	/// `core.cancelDeferred(model, handle, options)`.
	pub fn cancel_deferred(
		&self,
		request_model: &Model,
		handle: &DeferredHandle,
		cancel_options: Option<DeferredCancelOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		self.state.borrow_mut().cancelled_deferred.push(handle.clone());
		if let Some(entry) = self.inner.deferred_responses.borrow_mut().get_mut(&handle.id) {
			entry.cancelled = true;
		}
		let on_response = cancel_options.and_then(|options| options.on_response);
		let request_model = request_model.clone();
		Box::pin(async move {
			if let Some(on_response) = on_response {
				on_response(ok_response(), &request_model).await?;
			}
			Ok(())
		})
	}

	/// `getModel()` (the first model) or `getModel(modelId)`.
	pub fn get_model(&self, requested_model_id: Option<&str>) -> Option<Model> {
		match requested_model_id {
			None | Some("") => self.models.first().cloned(),
			Some(requested_model_id) => self
				.models
				.iter()
				.find(|candidate| candidate.id == requested_model_id)
				.cloned(),
		}
	}

	pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
		*self.inner.pending_responses.borrow_mut() = responses;
	}

	pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
		self.inner.pending_responses.borrow_mut().extend(responses);
	}

	pub fn get_pending_response_count(&self) -> usize {
		self.inner.pending_responses.borrow().len()
	}
}

/// The `api` that `fauxProvider()` gives `createProvider()`: the core's stream functions.
struct FauxStreams {
	core: FauxCore,
}

/// A `SimpleStreamOptions` key read off the `ProviderStreamOptions` record.
fn extra_option<T: DeserializeOwned>(extra: &JsonObject, key: &str) -> Option<T> {
	extra
		.get(key)
		.filter(|value| !value.is_null())
		.and_then(|value| serde_json::from_value(Value::clone(value)).ok())
}

/// `deferred` read off the record; a value `DeferredOption` cannot hold keeps its JS truthiness, which is all the
/// core's `if (streamOptions?.deferred)` reads.
fn extra_deferred(extra: &JsonObject) -> Option<DeferredOption> {
	let value = extra.get("deferred").filter(|value| !value.is_null())?;
	serde_json::from_value(value.clone()).ok().or_else(|| {
		Some(DeferredOption::Enabled(match value {
			Value::Bool(value) => *value,
			Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
			Value::String(value) => !value.is_empty(),
			_ => true,
		}))
	})
}

impl ProviderStreams for FauxStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let options = options.map(|options| SimpleStreamOptions {
			tool_choice: extra_option(&options.extra, "toolChoice"),
			reasoning: extra_option(&options.extra, "reasoning"),
			deferred: extra_deferred(&options.extra),
			thinking_budgets: extra_option(&options.extra, "thinkingBudgets"),
			base: options.base,
			extra: options.extra,
		});
		Ok(self.core.stream(model, context, options))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(self.core.stream_simple(model, context, options))
	}

	fn fetch_deferred(&self) -> Option<FetchDeferredFunction> {
		let core = self.core.clone();
		Some(Rc::new(move |model, handle, options| {
			Ok(core.fetch_deferred(model, handle, options))
		}))
	}

	fn cancel_deferred(&self) -> Option<CancelDeferredFunction> {
		let core = self.core.clone();
		Some(Rc::new(move |model, handle, options| {
			core.cancel_deferred(model, handle, options)
		}))
	}
}

/// Faux provider for tests built on explicit `Models` collections:
///
/// ```ignore
/// let faux = faux_provider(None);
/// let models = create_models(None);
/// models.set_provider(faux.provider.clone());
/// faux.set_responses(vec![faux_assistant_message("hi", None).into()]);
/// ```
pub fn faux_provider(options: Option<RegisterFauxProviderOptions>) -> FauxProviderHandle {
	let core = create_faux_core(options.unwrap_or_default());
	let provider = create_provider(CreateProviderOptions {
		id: core.provider.clone(),
		auth: ProviderAuth {
			api_key: Some(ApiKeyAuth {
				name: "Faux".to_string(),
				login: None,
				check: None,
				resolve: Rc::new(|_| {
					Box::pin(async {
						Ok(Some(AuthResult {
							auth: ModelAuth::default(),
							env: None,
							source: None,
						}))
					})
				}),
			}),
			oauth: None,
		},
		models: core.models.iter().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(Rc::new(FauxStreams { core: core.clone() }))),
		..CreateProviderOptions::default()
	})
	.expect("fauxProvider passes an api implementation");
	FauxProviderHandle {
		provider: Rc::new(provider),
		core,
	}
}
