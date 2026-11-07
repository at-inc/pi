//! Exports of `api/openai-responses-shared`.

use std::cell::RefCell;
use std::rc::Rc;

use futures::Stream;
use indexmap::IndexMap;
use js_sys::{Array, Function, Promise, Reflect};
use pi_ai::api::openai_responses_shared::{
	self, ApplyServiceTierPricing, ConvertResponsesMessagesOptions, ConvertResponsesToolsOptions,
	OnImageGenerationCall, OpenAIResponsesStreamOptions, ResolveServiceTier,
};
use pi_ai::{AssistantMessageEventStream, SharedAssistantMessage, Tool, Usage};
use pi_host::js::json_stringify;
use pi_host::{PiError, PiResult};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::events::{EventSink, Forwarder};
use crate::options::{arg, throw};

/// `getResponsesToolSupport(options)`: the flags are read for truthiness, as the TS reads them.
#[wasm_bindgen(js_name = aiGetResponsesToolSupport)]
pub fn get_responses_tool_support(options: JsValue) -> Result<JsValue, JsValue> {
	let flag = |name: &str| get(&options, name).is_truthy().then_some(true);
	let support = openai_responses_shared::get_responses_tool_support(
		flag("supportsMidConvoSystemMessages"),
		flag("supportsAdditionalTools"),
		flag("supportsToolSearch"),
	);
	throw(crate::json::to_js(&support))
}

/// `convertResponsesMessages(model, context, allowedToolCallProviders, options)`. The providers are any iterable of
/// strings (the TS takes a `Set`) and `options.grammarToolInputProperties` an iterable of entries (a `Map`). The items
/// cross through `JSON.parse`, which keeps a schema key such as `__proto__` an own property, as the TS objects have it.
#[wasm_bindgen(js_name = aiConvertResponsesMessages)]
pub fn convert_responses_messages(
	model: JsValue,
	context: JsValue,
	allowed_tool_call_providers: JsValue,
	options: JsValue,
) -> Result<JsValue, JsValue> {
	let model = super::model(&model)?;
	let context = super::transcript(context)?;
	let providers: Vec<String> = iterate(&allowed_tool_call_providers)?
		.iter()
		.map(|provider| provider.as_string().unwrap_or_default())
		.collect();
	let providers: Vec<&str> = providers.iter().map(String::as_str).collect();
	let options = if options.is_object() {
		Some(messages_options(&options)?)
	} else {
		None
	};
	let items = throw(openai_responses_shared::convert_responses_messages(
		&model,
		&context,
		&providers,
		options.as_ref(),
	))?;
	js_sys::JSON::parse(&json_stringify(&items))
}

/// `convertResponsesTools(tools, options)`, through `JSON.parse` like `convertResponsesMessages`.
#[wasm_bindgen(js_name = aiConvertResponsesTools)]
pub fn convert_responses_tools(tools: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
	let tools: Vec<Tool> = arg(tools)?;
	let options = options.is_object().then(|| tools_options(&options));
	let tools = throw(openai_responses_shared::convert_responses_tools(
		&tools,
		options.as_ref(),
	))?;
	js_sys::JSON::parse(&json_stringify(&tools))
}

/// `processResponsesStream(openaiStream, output, stream, model, options)`: a Promise that settles when the events of the
/// async iterable `events` are processed. The core fills a copy of `output`; `sink` keeps the caller's `output` as its
/// partial and pushes into the caller's stream. Before the next event is requested, the events the previous one
/// produced reach `sink`, so they see the message as it was after that event; at the end `sink` gets the whole
/// message, also when the processing rejects.
#[wasm_bindgen(js_name = aiProcessResponsesStream)]
pub fn process_responses_stream(
	events: JsValue,
	output: JsValue,
	model: JsValue,
	options: JsValue,
	sink: EventSink,
) -> Result<Promise, JsValue> {
	let parsed = super::model(&model)?;
	let output: SharedAssistantMessage = Rc::new(RefCell::new(arg(output)?));
	let options = if options.is_object() {
		Some(stream_options(&options, &model)?)
	} else {
		None
	};
	let stream = AssistantMessageEventStream::new();
	let forwarder = Rc::new(RefCell::new(Forwarder::new(stream.clone(), sink)));
	let source = js_events(events, forwarder.clone())?;
	Ok(wasm_bindgen_futures::future_to_promise(async move {
		let result =
			openai_responses_shared::process_responses_stream(source, &output, &stream, &parsed, options.as_ref())
				.await;
		forwarder.borrow_mut().finish(&output);
		result
			.map(|()| JsValue::UNDEFINED)
			.map_err(|error| crate::error::to_js(&error))
	}))
}

/// The events of a JS async iterable, read with its iterator's `next()`; `forwarder` flushes before each `next()`.
fn js_events(
	source: JsValue,
	forwarder: Rc<RefCell<Forwarder>>,
) -> Result<impl Stream<Item = PiResult<Value>> + Unpin, JsValue> {
	let iterator: Function = Reflect::get(&source, &js_sys::Symbol::async_iterator())?.dyn_into()?;
	let iterator = iterator.call0(&source)?;
	let next: Function = get(&iterator, "next").dyn_into()?;
	Ok(Box::pin(futures::stream::unfold(
		(iterator, next, forwarder),
		|(iterator, next, forwarder)| async move {
			forwarder.borrow_mut().flush();
			let step = match next.call0(&iterator) {
				Ok(step) => JsFuture::from(Promise::resolve(&step)).await,
				Err(error) => Err(error),
			};
			let event = match step {
				Err(error) => Err(crate::error::from_js(error)),
				Ok(step) if get(&step, "done").is_truthy() => return None,
				Ok(step) => crate::json::from_js(get(&step, "value")),
			};
			Some((event, (iterator, next, forwarder)))
		},
	)))
}

/// `ConvertResponsesMessagesOptions` of a JS options object.
fn messages_options(options: &JsValue) -> Result<ConvertResponsesMessagesOptions, JsValue> {
	let tool_options = get(options, "toolOptions");
	Ok(ConvertResponsesMessagesOptions {
		include_system_prompt: get(options, "includeSystemPrompt").as_bool(),
		grammar_tool_input_properties: string_map(&get(options, "grammarToolInputProperties"))?,
		supports_mid_convo_system_messages: get(options, "supportsMidConvoSystemMessages").as_bool(),
		supports_additional_tools: get(options, "supportsAdditionalTools").as_bool(),
		supports_tool_search: get(options, "supportsToolSearch").as_bool(),
		tool_options: tool_options.is_object().then(|| tools_options(&tool_options)),
		configuration_update_effort: get(options, "configurationUpdateEffort").as_string(),
	})
}

/// `ConvertResponsesToolsOptions` of a JS options object; `strict: null` stays apart from an absent `strict`.
fn tools_options(options: &JsValue) -> ConvertResponsesToolsOptions {
	let strict = get(options, "strict");
	ConvertResponsesToolsOptions {
		strict: (!strict.is_undefined()).then(|| strict.as_bool()),
		supports_strict_mode: get(options, "supportsStrictMode").as_bool(),
		supports_openai_grammar_tools: get(options, "supportsOpenAIGrammarTools").as_bool(),
		tool_search_result: get(options, "toolSearchResult").as_bool(),
	}
}

/// `OpenAIResponsesStreamOptions` of a JS options object. The callbacks are called with the options object as `this`,
/// as the TS calls them. The image and service tier callbacks are synchronous, as in the TS; what they throw is dropped,
/// as the Rust callbacks cannot fail.
fn stream_options(options: &JsValue, model: &JsValue) -> Result<OpenAIResponsesStreamOptions, JsValue> {
	let function = |name: &str| get(options, name).dyn_into::<Function>().ok();
	let image_callback = |function: Function| -> OnImageGenerationCall {
		let this = options.clone();
		Rc::new(move |index, value| {
			if let (Ok(index), Ok(value)) = (crate::json::to_js(index), crate::json::to_js(value)) {
				let _ = function.call2(&this, &index, &value);
			}
		})
	};
	let tier = |tier: Option<&str>| tier.map_or(JsValue::UNDEFINED, JsValue::from);
	Ok(OpenAIResponsesStreamOptions {
		on_provider_stream_event: function("onProviderStreamEvent")
			.map(|f| crate::options::on_provider_stream_event(f, options.clone(), model.clone())),
		service_tier: get(options, "serviceTier").as_string(),
		grammar_tool_input_properties: string_map(&get(options, "grammarToolInputProperties"))?,
		on_image_generation_call: function("onImageGenerationCall").map(image_callback),
		on_image_generation_partial_image: function("onImageGenerationPartialImage").map(image_callback),
		resolve_service_tier: function("resolveServiceTier").map(|function| {
			let this = options.clone();
			Rc::new(move |response: Option<&str>, request: Option<&str>| {
				function
					.call2(&this, &tier(response), &tier(request))
					.ok()
					.and_then(|tier| tier.as_string())
			}) as ResolveServiceTier
		}),
		apply_service_tier_pricing: function("applyServiceTierPricing").map(|function| {
			let this = options.clone();
			Rc::new(move |usage: &mut Usage, service_tier: Option<&str>| {
				let Ok(js_usage) = crate::json::to_js(usage) else {
					return;
				};
				if function.call2(&this, &js_usage, &tier(service_tier)).is_ok()
					&& let Ok(priced) = crate::json::from_js(js_usage)
				{
					*usage = priced;
				}
			}) as ApplyServiceTierPricing
		}),
	})
}

/// A `ReadonlyMap<string, string>` (any iterable of `[key, value]` entries); `None` for `undefined` and `null`.
fn string_map(value: &JsValue) -> Result<Option<IndexMap<String, String>>, JsValue> {
	if value.is_undefined() || value.is_null() {
		return Ok(None);
	}
	let mut map = IndexMap::new();
	for entry in iterate(value)? {
		let entry: Array = entry.unchecked_into();
		map.insert(
			entry.get(0).as_string().unwrap_or_default(),
			entry.get(1).as_string().unwrap_or_default(),
		);
	}
	Ok(Some(map))
}

/// The values of a JS iterable.
fn iterate(value: &JsValue) -> Result<Vec<JsValue>, JsValue> {
	let iterator =
		js_sys::try_iter(value)?.ok_or_else(|| crate::error::to_js(&PiError::type_error("value is not iterable")))?;
	iterator.collect()
}

fn get(target: &JsValue, key: &str) -> JsValue {
	Reflect::get(target, &key.into()).unwrap_or(JsValue::UNDEFINED)
}
