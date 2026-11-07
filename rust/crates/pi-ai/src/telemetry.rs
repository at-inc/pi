//! Port of `packages/ai/src/telemetry.ts`.
//! PORT-STATUS: reviewed
//!
//! `observeModelRequest`'s `options` (`{ telemetry?, reasoning?: unknown }`) becomes the two values it reads:
//! the span and the request's `reasoning` when it is a string. The `WeakMap` instrument cache is a thread-local
//! list of weak adapter references. Instruments may hold their adapter strongly, so an entry counts as dead, and is
//! pruned on the next lookup, once only its own instruments still hold the adapter. The 500-unit error message is cut
//! at a character boundary, so a surrogate pair straddling unit 500 is dropped where TS keeps its high half.
//! `AI_TELEMETRY_SCHEMA` is the typed pi-telemetry schema, which serializes attribute keys in struct order.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::LazyLock;

use futures::channel::oneshot;
use pi_host::js::utf16_len;
use pi_telemetry::{
	AttributeValue, NOOP_TELEMETRY_CONTEXT, NOOP_TELEMETRY_METRICS, SchemaTelemetryCounter, SchemaTelemetryHistogram,
	SchemaTelemetrySpan, SpanAttributes, SpanStatus, SpanStatusError, TelemetryContext, TelemetryMetrics,
	TelemetrySchemaDefinition, TelemetrySpan, create_typed_metrics, create_typed_span_starter, define_telemetry_schema,
};
use serde_json::{Value, json};

use crate::api::lazy::LazyStreamObserver;
use crate::types::{AssistantMessage, AssistantMessageEvent, Model, StopReason};
use crate::utils::text::owned_slice;

fn request_span_definition(description: &str) -> Value {
	let provider = json!({ "type": "string", "description": "Provider identifier.", "required": true });
	let model = json!({ "type": "string", "description": "Requested model identifier.", "required": true });
	json!({
		"description": description,
		"parents": { "kind": "any" },
		"startAttributes": {
			"provider": provider,
			"api": { "type": "string", "description": "API identifier.", "required": true },
			"model": model,
			"reasoning": { "type": "string", "description": "Scalar requested reasoning level.", "required": false },
		},
		"endAttributes": {
			"stop": stop_definition(),
			"usage.input": { "type": "number", "description": "Uncached input tokens." },
			"usage.output": { "type": "number", "description": "Output tokens." },
			"usage.cache_read": { "type": "number", "description": "Cache-read tokens." },
			"usage.cache_write": { "type": "number", "description": "Cache-write tokens." },
			"cost.total": { "type": "number", "description": "Total request cost in USD." },
			"response.id": { "type": "string", "description": "Provider response identifier.", "cardinality": "high" },
		},
		"events": {
			"pi.ai.first_chunk": { "description": "First normalized stream event delivered.", "attributes": {} },
			"pi.ai.block": {
				"description": "A completed content block; content is never recorded.",
				"attributes": {
					"type": {
						"type": "string",
						"description": "Block kind.",
						"values": ["text", "thinking", "tool_call"],
						"required": true,
					},
					"index": { "type": "number", "description": "Content block index.", "required": true },
					"chars": { "type": "number", "description": "UTF-16 character count.", "required": true },
					"tool": { "type": "string", "description": "Tool name, for tool-call blocks only.", "required": false },
				},
			},
		},
		"status": {
			"default": "ok",
			"errorWhen": "Stop reason is error: provider_error with at most 500 error-message characters. Aborted stays ok.",
		},
	})
}

fn stop_definition() -> Value {
	json!({
		"type": "string",
		"description": "Normalized stop reason.",
		"values": ["pending", "stop", "length", "toolUse", "error", "aborted", "deferred"],
	})
}

/// Pi-owned request metadata, bounded error status, events, and low-cardinality metrics.
pub static AI_TELEMETRY_SCHEMA: LazyLock<TelemetrySchemaDefinition> = LazyLock::new(|| {
	let provider = json!({ "type": "string", "description": "Provider identifier.", "required": true });
	let model = json!({ "type": "string", "description": "Requested model identifier.", "required": true });
	let mut required_stop = stop_definition();
	required_stop["required"] = Value::Bool(true);
	let schema = json!({
		"version": 1,
		"spans": {
			"pi.ai.request": request_span_definition("One logical model request, including authentication and provider setup."),
			"pi.ai.poll": request_span_definition("One deferred-response poll, not the deferred request's lifetime."),
		},
		"metrics": {
			"pi.ai.request.duration": {
				"kind": "histogram",
				"description": "Logical request or poll duration.",
				"unit": "ms",
				"attributes": { "provider": provider, "model": model, "stop": required_stop },
			},
			"pi.ai.first_chunk.duration": {
				"kind": "histogram",
				"description": "Time to the first normalized stream event.",
				"unit": "ms",
				"attributes": { "provider": provider, "model": model },
			},
			"pi.ai.tokens": {
				"kind": "counter",
				"description": "Reported model token usage; pending and deferred polls are excluded.",
				"unit": "token",
				"attributes": {
					"provider": provider,
					"model": model,
					"kind": {
						"type": "string",
						"description": "Token usage category.",
						"values": ["input", "output", "cache_read", "cache_write"],
						"required": true,
					},
				},
			},
			"pi.ai.cost": {
				"kind": "counter",
				"description": "Reported model cost; pending and deferred polls are excluded.",
				"unit": "USD",
				"attributes": { "provider": provider, "model": model },
			},
		},
	});
	define_telemetry_schema(serde_json::from_value(schema).expect("AI_TELEMETRY_SCHEMA is a valid telemetry schema"))
});

#[derive(Clone)]
struct Instruments {
	request_duration: Rc<SchemaTelemetryHistogram>,
	first_chunk_duration: Rc<SchemaTelemetryHistogram>,
	tokens: Rc<SchemaTelemetryCounter>,
	cost: Rc<SchemaTelemetryCounter>,
}

fn create_instruments(adapter: Rc<dyn TelemetryMetrics>) -> Instruments {
	let metrics = create_typed_metrics(adapter, std::slice::from_ref(&*AI_TELEMETRY_SCHEMA));
	Instruments {
		request_duration: metrics.histogram("pi.ai.request.duration"),
		first_chunk_duration: metrics.histogram("pi.ai.first_chunk.duration"),
		tokens: metrics.counter("pi.ai.tokens"),
		cost: metrics.counter("pi.ai.cost"),
	}
}

/// A cached adapter, its instruments, and how many strong references to the adapter those instruments hold.
struct CachedInstruments {
	adapter: Weak<dyn TelemetryMetrics>,
	instruments: Instruments,
	self_references: usize,
}

thread_local! {
	static INSTRUMENT_CACHE: RefCell<Vec<CachedInstruments>> = const { RefCell::new(Vec::new()) };
}

fn cached_instruments(adapter: Rc<dyn TelemetryMetrics>) -> Instruments {
	INSTRUMENT_CACHE.with(|cache| {
		let mut cache = cache.borrow_mut();
		// A `WeakMap` value does not keep its key alive. Instruments may point back at their adapter, so an entry is
		// dead once nothing but its own instruments holds the adapter.
		cache.retain(|entry| entry.adapter.strong_count() > entry.self_references);
		let key = Rc::downgrade(&adapter);
		if let Some(entry) = cache.iter().find(|entry| Weak::ptr_eq(&entry.adapter, &key)) {
			return entry.instruments.clone();
		}
		let before = Rc::strong_count(&adapter);
		let instruments = create_instruments(adapter.clone());
		let self_references = Rc::strong_count(&adapter).saturating_sub(before);
		cache.push(CachedInstruments {
			adapter: key,
			instruments: instruments.clone(),
			self_references,
		});
		instruments
	})
}

fn stop_reason_str(reason: StopReason) -> String {
	match serde_json::to_value(reason) {
		Ok(Value::String(reason)) => reason,
		_ => unreachable!("StopReason serializes as a string"),
	}
}

fn attributes<const N: usize>(entries: [(&str, Option<AttributeValue>); N]) -> SpanAttributes {
	entries
		.into_iter()
		.map(|(name, value)| (name.to_string(), value))
		.collect()
}

/// Observes one model request or deferred poll: a `pi.ai.request` / `pi.ai.poll` span with block events, plus
/// duration, token and cost metrics.
pub fn observe_model_request(
	requested_model: &Model,
	telemetry: Option<Rc<dyn TelemetrySpan>>,
	reasoning: Option<&str>,
	poll: bool,
) -> LazyStreamObserver {
	let started = pi_host::monotonic_ms();
	let provider = requested_model.provider.to_string();
	let model = requested_model.id.clone();
	let adapter = telemetry
		.as_ref()
		.and_then(|telemetry| telemetry.metrics())
		.unwrap_or_else(|| NOOP_TELEMETRY_METRICS.with(Rc::clone));
	let instruments = cached_instruments(adapter);
	let span: Rc<RefCell<Option<Rc<SchemaTelemetrySpan>>>> = Rc::new(RefCell::new(None));
	let first = Rc::new(RefCell::new(true));
	let tool_chars: Rc<RefCell<HashMap<usize, usize>>> = Rc::new(RefCell::new(HashMap::new()));
	let (finish, finished) = oneshot::channel::<()>();
	let finish = Rc::new(RefCell::new(Some(finish)));

	let mut start_attributes = attributes([
		("provider", Some(provider.as_str().into())),
		("model", Some(model.as_str().into())),
		("api", Some(AttributeValue::from(&*requested_model.api))),
	]);
	if let Some(reasoning) = reasoning {
		start_attributes.insert("reasoning".into(), Some(reasoning.into()));
	}
	let context: Rc<dyn TelemetryContext> = match telemetry {
		Some(telemetry) => telemetry,
		None => NOOP_TELEMETRY_CONTEXT.with(Rc::clone),
	};
	let start = create_typed_span_starter(context, std::slice::from_ref(&*AI_TELEMETRY_SCHEMA));
	let active_span = span.clone();
	let run = move |span: Rc<SchemaTelemetrySpan>, _| {
		*active_span.borrow_mut() = Some(span);
		async move {
			if finished.await.is_err() {
				std::future::pending::<()>().await;
			}
			Ok(())
		}
	};
	let request = start.start_span(if poll { "pi.ai.poll" } else { "pi.ai.request" }, start_attributes, run);
	pi_host::spawn_local(async move {
		let _ = request.await;
	});

	let current_span = |span: &Rc<RefCell<Option<Rc<SchemaTelemetrySpan>>>>| {
		span.borrow()
			.clone()
			.expect("startSpan runs its callback before returning")
	};
	let base_attributes = {
		let (provider, model) = (provider.clone(), model.clone());
		move || {
			attributes([
				("provider", Some(provider.as_str().into())),
				("model", Some(model.as_str().into())),
			])
		}
	};

	let on_event = {
		let (span, instruments, base_attributes) = (span.clone(), instruments.clone(), base_attributes.clone());
		move |event: &AssistantMessageEvent| {
			let span = current_span(&span);
			if first.replace(false) {
				span.add_event("pi.ai.first_chunk", None);
				instruments.first_chunk_duration.record(
					pi_host::monotonic_ms() - started,
					Some(&base_attributes()),
					Some(&*span as &dyn TelemetryContext),
				);
			}
			match event {
				AssistantMessageEvent::ToolcallDelta {
					content_index, delta, ..
				} => {
					*tool_chars.borrow_mut().entry(*content_index).or_insert(0) += utf16_len(delta);
				}
				AssistantMessageEvent::TextEnd {
					content_index, content, ..
				}
				| AssistantMessageEvent::ThinkingEnd {
					content_index, content, ..
				} => {
					let kind = if matches!(event, AssistantMessageEvent::TextEnd { .. }) {
						"text"
					} else {
						"thinking"
					};
					span.add_event(
						"pi.ai.block",
						Some(&attributes([
							("type", Some(kind.into())),
							("index", Some((*content_index as f64).into())),
							("chars", Some((utf16_len(content) as f64).into())),
						])),
					);
				}
				AssistantMessageEvent::ToolcallEnd {
					content_index,
					tool_call,
					..
				} => {
					let chars = tool_chars.borrow_mut().remove(content_index).unwrap_or(0);
					span.add_event(
						"pi.ai.block",
						Some(&attributes([
							("type", Some("tool_call".into())),
							("index", Some((*content_index as f64).into())),
							("chars", Some((chars as f64).into())),
							("tool", Some(tool_call.name.as_str().into())),
						])),
					);
				}
				_ => {}
			}
		}
	};

	let on_result = move |message: Option<&AssistantMessage>| {
		if let Some(message) = message {
			let span = current_span(&span);
			let stop = stop_reason_str(message.stop_reason);
			span.set_attributes(&attributes([
				("stop", Some(stop.as_str().into())),
				("usage.input", Some(message.usage.input.into())),
				("usage.output", Some(message.usage.output.into())),
				("usage.cache_read", Some(message.usage.cache_read.into())),
				("usage.cache_write", Some(message.usage.cache_write.into())),
				("cost.total", Some(message.usage.cost.total.into())),
				("response.id", message.response_id.as_deref().map(AttributeValue::from)),
			]));
			if message.stop_reason == StopReason::Error {
				span.set_status(&SpanStatus::Error {
					error: Some(SpanStatusError {
						name: "provider_error".into(),
						message: owned_slice(message.error_message.as_deref().unwrap_or(""), 0, Some(500)),
					}),
				});
			}
			let mut duration_attributes = base_attributes();
			duration_attributes.insert("stop".into(), Some(stop.as_str().into()));
			instruments.request_duration.record(
				pi_host::monotonic_ms() - started,
				Some(&duration_attributes),
				Some(&*span as &dyn TelemetryContext),
			);
			if !poll || (message.stop_reason != StopReason::Pending && message.stop_reason != StopReason::Deferred) {
				for (value, kind) in [
					(message.usage.input, "input"),
					(message.usage.output, "output"),
					(message.usage.cache_read, "cache_read"),
					(message.usage.cache_write, "cache_write"),
				] {
					let mut token_attributes = base_attributes();
					token_attributes.insert("kind".into(), Some(kind.into()));
					instruments.tokens.add(value, Some(&token_attributes));
				}
				instruments.cost.add(message.usage.cost.total, Some(&base_attributes()));
			}
		}
		if let Some(finish) = finish.borrow_mut().take() {
			let _ = finish.send(());
		}
	};

	LazyStreamObserver {
		on_event: Some(Rc::new(on_event)),
		on_result: Some(Rc::new(on_result)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn instrument_cache_lets_a_dropped_adapter_go() {
		let telemetry = pi_telemetry::InMemoryTelemetryContext::new();
		let adapter: Rc<dyn TelemetryMetrics> = telemetry.clone();
		let first = cached_instruments(adapter.clone());
		let again = cached_instruments(adapter.clone());
		assert!(Rc::ptr_eq(&first.tokens, &again.tokens));
		let weak = Rc::downgrade(&telemetry);
		drop((first, again, adapter, telemetry));
		cached_instruments(pi_telemetry::InMemoryTelemetryContext::new());
		assert!(weak.upgrade().is_none());
	}
}
