//! Port of `packages/ai/test/telemetry.test.ts`.
//!
//! Differences from the TS:
//! - Every test that sends a request through `Models` with provider auth (each `setup()` test and "defaults to no-op
//!   telemetry") is `#[ignore = "needs w2-ai-auth: resolve_provider_auth"]`: `Models` resolves auth through the stub
//!   on this branch. They pass against the auth branch.
//! - `it.each` cases are one test fn each.
//! - Event identity (`toBe`) is `Rc::ptr_eq` on the shared message each event carries. Result identity is equality
//!   without `duration_ms` (`unstamped`): `result()` returns a copy, and the event stream stamps `durationMs` on that
//!   copy where TS stamps the caller's own object.
//! - "keeps a bounded error message that does not hold the provider's whole message alive" (#9265) checks only the
//!   500-character status message. It skips the heap measurement: Rust has no GC heap statistics, and the status
//!   message is an owned copy that cannot keep the provider's message alive.
//! - "counts interleaved tool deltas independently without reading arguments" cannot give the tool call an
//!   `arguments` getter that throws, because `ToolCall.arguments` is plain data. It keeps the deltas and the counts.
//! - `{ ...span, metrics: undefined }` is `TracingOnlySpan`, which delegates to the span and has no metrics.
//! - `vi.spyOn(telemetry, "histogram" | "counter")` is `SpyingSpan`: a delegating span whose metrics adapter records
//!   instrument names and then delegates to the in-memory adapter.
//! - `vi.spyOn(performance, "now")` is the test host's virtual clock, moved with `TestHost::advance`.
//! - "uses one source iterator and observes after push, once per terminal result": the source is moved into
//!   `lazy_stream`, so the one-iterator count holds by construction and is not asserted. The `.then` that `onEvent`
//!   attaches is a task spawned before the stream and woken by `on_event`, so it is queued at `onEvent` time like the
//!   TS microtask. A task spawned inside `on_event` would join the test host's `LocalPool` only after the forwarding
//!   task has yielded, so it would run after `onResult`.
//! - `vi.waitFor` is `wait_for`, which polls every 50 ms of host time for up to 1 s like vitest's defaults.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot;
use pi_ai::api::lazy::{LazySource, LazyStreamObserver, lazy_stream};
use pi_ai::auth::types::{ApiKeyAuth, AuthResult, ModelAuth, ProviderAuth};
use pi_ai::models::{
	CreateProviderOptions, ModelsApiStreamOptions, ModelsDeferredFetchOptions, ModelsSimpleStreamOptions,
	MutableModels, ProviderApi, create_models, create_provider,
};
use pi_ai::telemetry::{AI_TELEMETRY_SCHEMA, observe_model_request};
use pi_ai::types::{
	AnyModel, AssistantContent, AssistantMessage, AssistantMessageEvent, BaseModel, Context, DeferredFetchOptions,
	DeferredHandle, FetchDeferredFunction, Model, ModelCost, ModelInputModality, ProviderRequestOptions,
	ProviderStreamOptions, ProviderStreams, SharedAssistantMessage, SimpleStreamOptions, StopReason, StreamOptions,
	TextContent, ThinkingContent, ThinkingLevel, ToolCall, TranscriptContext, Usage, UsageCost,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::testing::run;
use pi_host::{LocalBoxFuture, PiError, PiResult};
use pi_telemetry::{
	HistogramOptions, InMemoryTelemetryContext, InstrumentOptions, MetricKind, SpanAttributes, SpanCallback, SpanLink,
	SpanOptions, SpanStatus, SpanStatusError, TelemetryContext, TelemetryContextExt, TelemetryCounter,
	TelemetryHistogram, TelemetryMetrics, TelemetryPropagation, TelemetrySpan, TelemetryUpDownCounter,
};
use serde_json::{Value, json};

fn model() -> Model {
	Model {
		base: BaseModel {
			id: "model".into(),
			name: "Model".into(),
			api: "telemetry-test".into(),
			provider: "test".into(),
			base_url: "https://example.test".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost {
				input: 1.0,
				output: 2.0,
				cache_read: 3.0,
				cache_write: 4.0,
				tiers: None,
				..ModelCost::default()
			},
			..BaseModel::default()
		},
		reasoning: true,
		context_window: 1000.0,
		max_tokens: 100.0,
		..Model::default()
	}
}

fn handle() -> DeferredHandle {
	DeferredHandle {
		provider: "test".into(),
		model_id: "model".into(),
		api: "telemetry-test".into(),
		id: "deferred-secret".into(),
		..DeferredHandle::default()
	}
}

fn message(stop_reason: StopReason) -> AssistantMessage {
	AssistantMessage {
		content: vec![
			AssistantContent::Text(TextContent {
				text: "text-secret".into(),
				..TextContent::default()
			}),
			AssistantContent::Thinking(ThinkingContent {
				thinking: "thinking-secret".into(),
				..ThinkingContent::default()
			}),
			AssistantContent::ToolCall(ToolCall {
				id: "call-secret".into(),
				name: "lookup".into(),
				arguments: json!({ "value": "argument-secret" }).as_object().cloned().unwrap(),
				..ToolCall::default()
			}),
		],
		api: "telemetry-test".into(),
		provider: "test".into(),
		model: "model".into(),
		usage: Usage {
			input: 11.0,
			output: 22.0,
			cache_read: 33.0,
			cache_write: 44.0,
			total_tokens: 110.0,
			cost: UsageCost {
				input: 0.1,
				output: 0.2,
				cache_read: 0.3,
				cache_write: 0.4,
				total: 1.0,
				..UsageCost::default()
			},
			..Usage::default()
		},
		response_id: Some("response".into()),
		stop_reason,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

fn events(result: &AssistantMessage) -> Vec<AssistantMessageEvent> {
	let AssistantContent::ToolCall(tool_call) = &result.content[2] else {
		panic!("Missing tool call");
	};
	let partial = Rc::new(RefCell::new(result.clone()));
	let terminal = match result.stop_reason {
		StopReason::Error | StopReason::Aborted => AssistantMessageEvent::Error {
			reason: result.stop_reason,
			error: partial.clone(),
		},
		reason => AssistantMessageEvent::Done {
			reason: if reason == StopReason::Pending {
				StopReason::Deferred
			} else {
				reason
			},
			message: partial.clone(),
		},
	};
	let delta = |content_index: usize, delta: &str| AssistantMessageEvent::ToolcallDelta {
		content_index,
		delta: delta.into(),
		partial: partial.clone(),
	};
	vec![
		AssistantMessageEvent::Start {
			partial: partial.clone(),
		},
		AssistantMessageEvent::TextDelta {
			content_index: 0,
			delta: "text-secret".into(),
			partial: partial.clone(),
		},
		AssistantMessageEvent::TextEnd {
			content_index: 0,
			content: "text-secret".into(),
			partial: partial.clone(),
		},
		AssistantMessageEvent::ThinkingEnd {
			content_index: 1,
			content: "thinking-secret".into(),
			partial: partial.clone(),
		},
		delta(2, "{\"value\":"),
		delta(2, "\"argument-secret\"}"),
		AssistantMessageEvent::ToolcallEnd {
			content_index: 2,
			tool_call: tool_call.clone(),
			partial: partial.clone(),
		},
		terminal,
	]
}

/// The shared message an event carries, for `toBe` identity checks.
fn shared_message(event: &AssistantMessageEvent) -> &SharedAssistantMessage {
	match event {
		AssistantMessageEvent::Start { partial }
		| AssistantMessageEvent::TextStart { partial, .. }
		| AssistantMessageEvent::TextDelta { partial, .. }
		| AssistantMessageEvent::TextEnd { partial, .. }
		| AssistantMessageEvent::ThinkingStart { partial, .. }
		| AssistantMessageEvent::ThinkingDelta { partial, .. }
		| AssistantMessageEvent::ThinkingEnd { partial, .. }
		| AssistantMessageEvent::ToolcallStart { partial, .. }
		| AssistantMessageEvent::ToolcallDelta { partial, .. }
		| AssistantMessageEvent::ToolcallEnd { partial, .. } => partial,
		AssistantMessageEvent::Done { message, .. } => message,
		AssistantMessageEvent::Error { error, .. } => error,
	}
}

fn respond(result: &AssistantMessage, events: &[AssistantMessageEvent]) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	for event in events {
		stream.push(event.clone());
	}
	stream.end(Some(result.clone()));
	stream
}

/// `{ stream: respond, streamSimple: respond, fetchDeferred: respond }`.
struct Respond {
	result: AssistantMessage,
	events: Vec<AssistantMessageEvent>,
}

impl ProviderStreams for Respond {
	fn stream(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		_options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(respond(&self.result, &self.events))
	}

	fn stream_simple(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		_options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(respond(&self.result, &self.events))
	}

	fn fetch_deferred(&self) -> Option<FetchDeferredFunction> {
		let (result, events) = (self.result.clone(), self.events.clone());
		Some(Rc::new(
			move |_: &Model, _: &DeferredHandle, _: Option<DeferredFetchOptions>| Ok(respond(&result, &events)),
		))
	}
}

struct Setup {
	models: Rc<dyn MutableModels>,
	stream_events: Vec<AssistantMessageEvent>,
}

fn setup(result: AssistantMessage) -> Setup {
	let models = create_models(None);
	let stream_events = events(&result);
	let provider = create_provider(CreateProviderOptions {
		id: "test".into(),
		models: vec![AnyModel::Chat(model())],
		auth: ProviderAuth {
			api_key: Some(ApiKeyAuth {
				name: "Test".into(),
				login: None,
				check: None,
				resolve: Rc::new(|_| {
					Box::pin(async {
						Ok(Some(AuthResult {
							auth: ModelAuth::default(),
							..AuthResult::default()
						}))
					})
				}),
			}),
			oauth: None,
		},
		api: Some(ProviderApi::Single(Rc::new(Respond {
			result,
			events: stream_events.clone(),
		}))),
		..CreateProviderOptions::default()
	})
	.expect("the test provider is valid");
	models.set_provider(Rc::new(provider));
	Setup { models, stream_events }
}

fn context() -> Context {
	Context::default()
}

fn request_options(telemetry: Rc<dyn TelemetrySpan>) -> ProviderRequestOptions<Model> {
	ProviderRequestOptions {
		telemetry: Some(telemetry),
		..ProviderRequestOptions::default()
	}
}

fn simple_options(
	telemetry: Rc<dyn TelemetrySpan>,
	reasoning: Option<ThinkingLevel>,
) -> Option<ModelsSimpleStreamOptions> {
	Some(
		SimpleStreamOptions {
			base: StreamOptions {
				base: request_options(telemetry),
				..StreamOptions::default()
			},
			reasoning,
			..SimpleStreamOptions::default()
		}
		.into(),
	)
}

fn api_options(telemetry: Rc<dyn TelemetrySpan>, reasoning: Option<Value>) -> Option<ModelsApiStreamOptions> {
	Some(
		ProviderStreamOptions {
			base: StreamOptions {
				base: request_options(telemetry),
				..StreamOptions::default()
			},
			extra: reasoning
				.map(|reasoning| [("reasoning".to_string(), reasoning)].into_iter().collect())
				.unwrap_or_default(),
		}
		.into(),
	)
}

fn deferred_options(telemetry: Rc<dyn TelemetrySpan>) -> Option<ModelsDeferredFetchOptions> {
	Some(
		DeferredFetchOptions {
			base: request_options(telemetry),
			wait: None,
		}
		.into(),
	)
}

fn caller() -> SpanOptions {
	SpanOptions {
		name: "caller".into(),
		..SpanOptions::default()
	}
}

/// A result without the `duration_ms` the event stream stamps on it.
fn unstamped(result: Option<AssistantMessage>) -> Option<AssistantMessage> {
	result.map(|result| AssistantMessage {
		duration_ms: None,
		..result
	})
}

fn json_of(value: &impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

/// `vi.waitFor(check)` with vitest's defaults: retry every 50 ms for up to 1 s.
async fn wait_for(check: impl Fn() -> bool) {
	for _ in 0..20 {
		if check() {
			return;
		}
		pi_host::sleep(50.0, None).await.unwrap();
	}
	assert!(check(), "wait_for timed out");
}

async fn settled(telemetry: &InMemoryTelemetryContext) {
	wait_for(|| telemetry.get_spans().iter().all(|span| span.settled)).await;
}

/// `{ ...span, metrics: undefined }`: the span without metric capabilities.
struct TracingOnlySpan {
	span: Rc<dyn TelemetrySpan>,
}

impl TelemetryContext for TracingOnlySpan {
	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		self.span.propagation()
	}

	fn start_span_erased(
		&self,
		options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
		self.span.start_span_erased(options, callback)
	}
}

impl TelemetrySpan for TracingOnlySpan {
	fn add_link(&self, link: &SpanLink) {
		self.span.add_link(link);
	}

	fn add_event(&self, name: &str, attributes: Option<&SpanAttributes>) {
		self.span.add_event(name, attributes);
	}

	fn set_attributes(&self, attributes: &SpanAttributes) {
		self.span.set_attributes(attributes);
	}

	fn set_status(&self, status: &SpanStatus) {
		self.span.set_status(status);
	}
}

/// `vi.spyOn(telemetry, "histogram")` and `vi.spyOn(telemetry, "counter")` on the in-memory adapter.
struct SpyMetrics {
	adapter: Rc<InMemoryTelemetryContext>,
	histograms: RefCell<Vec<String>>,
	counters: RefCell<Vec<String>>,
}

impl TelemetryMetrics for SpyMetrics {
	fn counter(&self, name: &str, options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryCounter> {
		self.counters.borrow_mut().push(name.into());
		self.adapter.counter(name, options)
	}

	fn up_down_counter(&self, name: &str, options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryUpDownCounter> {
		self.adapter.up_down_counter(name, options)
	}

	fn histogram(&self, name: &str, options: Option<&HistogramOptions>) -> Rc<dyn TelemetryHistogram> {
		self.histograms.borrow_mut().push(name.into());
		self.adapter.histogram(name, options)
	}
}

/// A span whose metrics are the spied adapter.
struct SpyingSpan {
	span: Rc<dyn TelemetrySpan>,
	metrics: Rc<SpyMetrics>,
}

impl TelemetryContext for SpyingSpan {
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		Some(self.metrics.clone())
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		self.span.propagation()
	}

	fn start_span_erased(
		&self,
		options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
		self.span.start_span_erased(options, callback)
	}
}

impl TelemetrySpan for SpyingSpan {
	fn add_link(&self, link: &SpanLink) {
		self.span.add_link(link);
	}

	fn add_event(&self, name: &str, attributes: Option<&SpanAttributes>) {
		self.span.add_event(name, attributes);
	}

	fn set_attributes(&self, attributes: &SpanAttributes) {
		self.span.set_attributes(attributes);
	}

	fn set_status(&self, status: &SpanStatus) {
		self.span.set_status(status);
	}
}

mod ai_telemetry {
	use super::*;

	#[derive(Clone, Copy, PartialEq)]
	enum Method {
		Stream,
		StreamSimple,
		StreamDeferred,
	}

	fn observes_without_changing_delivered_events_or_results(method: Method) {
		run(|_| async move {
			let telemetry = InMemoryTelemetryContext::new();
			let result = message(StopReason::Stop);
			let Setup { models, stream_events } = setup(result.clone());
			let expected_result = result.clone();
			telemetry
				.start_span(caller(), move |parent| async move {
					let stream = match method {
						Method::StreamDeferred => models.stream_deferred(&model(), &handle(), deferred_options(parent)),
						Method::Stream => models.stream(&model(), &context(), api_options(parent, Some(json!("high")))),
						Method::StreamSimple => models.stream_simple(
							&model(),
							&context(),
							simple_options(parent, Some(ThinkingLevel::High)),
						),
					};
					let mut received = Vec::new();
					while let Some(event) = stream.next().await {
						received.push(event);
					}
					assert_eq!(received, stream_events);
					for (index, event) in received.iter().enumerate() {
						assert!(Rc::ptr_eq(shared_message(event), shared_message(&stream_events[index])));
					}
					assert_eq!(unstamped(stream.result().await), Some(expected_result));
					Ok(())
				})
				.await
				.unwrap();
			settled(&telemetry).await;
			let spans = telemetry.get_spans();
			let [parent, span] = &spans[..] else {
				panic!("expected two spans, got {}", spans.len());
			};
			assert_eq!(
				span.name,
				if method == Method::StreamDeferred {
					"pi.ai.poll"
				} else {
					"pi.ai.request"
				}
			);
			assert_eq!(span.parent_id, Some(parent.id));
			let mut attributes = json!({
				"provider": "test",
				"api": "telemetry-test",
				"model": "model",
				"stop": "stop",
				"usage.input": 11.0,
				"usage.output": 22.0,
				"usage.cache_read": 33.0,
				"usage.cache_write": 44.0,
				"cost.total": 1.0,
				"response.id": "response",
			});
			if method != Method::StreamDeferred {
				attributes["reasoning"] = json!("high");
			}
			assert_eq!(json_of(&span.attributes), attributes);
			assert_eq!(
				json_of(&span.events),
				json!([
					{ "name": "pi.ai.first_chunk", "attributes": {} },
					{ "name": "pi.ai.block", "attributes": { "type": "text", "index": 0.0, "chars": 11.0 } },
					{ "name": "pi.ai.block", "attributes": { "type": "thinking", "index": 1.0, "chars": 15.0 } },
					{
						"name": "pi.ai.block",
						"attributes": { "type": "tool_call", "index": 2.0, "chars": 27.0, "tool": "lookup" },
					},
				])
			);
			let serialized = serde_json::to_string(span).unwrap();
			for secret in [
				"text-secret",
				"thinking-secret",
				"argument-secret",
				"call-secret",
				"deferred-secret",
			] {
				assert!(!serialized.contains(secret), "span leaks {secret}");
			}
			let measurements = telemetry.get_metrics();
			assert_eq!(measurements.len(), 7);
			assert_eq!(
				measurements
					.iter()
					.filter(|measurement| measurement.kind == MetricKind::Counter)
					.map(|measurement| measurement.value)
					.collect::<Vec<_>>(),
				vec![11.0, 22.0, 33.0, 44.0, 1.0]
			);
			for measurement in &measurements {
				let mut keys: Vec<&str> = measurement.attributes.keys().map(String::as_str).collect();
				keys.sort();
				assert_eq!(
					keys,
					match measurement.name.as_str() {
						"pi.ai.tokens" => vec!["kind", "model", "provider"],
						"pi.ai.request.duration" => vec!["model", "provider", "stop"],
						_ => vec!["model", "provider"],
					}
				);
				if measurement.kind == MetricKind::Histogram {
					assert_eq!(measurement.context.as_ref(), Some(&span.context));
				}
			}
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn observes_stream_without_changing_delivered_events_or_results() {
		observes_without_changing_delivered_events_or_results(Method::Stream);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn observes_stream_simple_without_changing_delivered_events_or_results() {
		observes_without_changing_delivered_events_or_results(Method::StreamSimple);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn observes_stream_deferred_without_changing_delivered_events_or_results() {
		observes_without_changing_delivered_events_or_results(Method::StreamDeferred);
	}

	fn reports_status_without_error_attributes_or_events(reason: StopReason) {
		run(|_| async move {
			let telemetry = InMemoryTelemetryContext::new();
			let result = AssistantMessage {
				error_message: Some("error-secret".into()),
				..message(reason)
			};
			let Setup { models, .. } = setup(result.clone());
			let completed = telemetry
				.start_span(caller(), move |span| async move {
					Ok(models
						.complete_simple(&model(), &context(), simple_options(span, None))
						.await)
				})
				.await
				.unwrap();
			assert_eq!(unstamped(completed), Some(result));
			settled(&telemetry).await;
			let span = telemetry.get_spans().remove(1);
			assert_eq!(
				span.status,
				if reason == StopReason::Error {
					SpanStatus::Error {
						error: Some(SpanStatusError {
							name: "provider_error".into(),
							message: "error-secret".into(),
						}),
					}
				} else {
					SpanStatus::Ok
				}
			);
			assert_eq!(
				json_of(&span.attributes["stop"]),
				json_of(&serde_json::to_value(reason).unwrap())
			);
			assert!(
				!serde_json::to_string(&span.attributes)
					.unwrap()
					.contains("error-secret")
			);
			assert!(!serde_json::to_string(&span.events).unwrap().contains("error-secret"));
			if reason == StopReason::Aborted {
				assert!(!serde_json::to_string(&span).unwrap().contains("error-secret"));
			}
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn reports_error_status_without_error_attributes_or_events() {
		reports_status_without_error_attributes_or_events(StopReason::Error);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn reports_aborted_status_without_error_attributes_or_events() {
		reports_status_without_error_attributes_or_events(StopReason::Aborted);
	}

	fn bounds_poll_error_messages(missing: bool) {
		run(|_| async move {
			let telemetry = InMemoryTelemetryContext::new();
			let mut result = message(StopReason::Error);
			if !missing {
				result.error_message = Some(format!("{}discarded-tail", "x".repeat(500)));
			}
			let Setup { models, .. } = setup(result.clone());
			let fetched = telemetry
				.start_span(caller(), move |span| async move {
					Ok(models.fetch_deferred(&model(), &handle(), deferred_options(span)).await)
				})
				.await
				.unwrap();
			assert_eq!(unstamped(fetched), Some(result));
			settled(&telemetry).await;
			let span = telemetry.get_spans().remove(1);
			assert_eq!(
				span.status,
				SpanStatus::Error {
					error: Some(SpanStatusError {
						name: "provider_error".into(),
						message: if missing { String::new() } else { "x".repeat(500) },
					}),
				}
			);
			assert!(!serde_json::to_string(&span).unwrap().contains("discarded-tail"));
			assert!(!serde_json::to_string(&span.attributes).unwrap().contains("xxx"));
			assert!(!serde_json::to_string(&span.events).unwrap().contains("xxx"));
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn bounds_poll_error_messages_including_missing_messages_missing_false() {
		bounds_poll_error_messages(false);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn bounds_poll_error_messages_including_missing_messages_missing_true() {
		bounds_poll_error_messages(true);
	}

	// #9265
	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn keeps_a_bounded_error_message_that_does_not_hold_the_providers_whole_message_alive() {
		run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			{
				let mut result = message(StopReason::Error);
				result.error_message = Some(format!("{}tail", "x".repeat(4_000_000)));
				let Setup { models, .. } = setup(result);
				telemetry
					.start_span(caller(), move |span| async move {
						Ok(models.fetch_deferred(&model(), &handle(), deferred_options(span)).await)
					})
					.await
					.unwrap();
				settled(&telemetry).await;
			}
			let span = telemetry.get_spans().into_iter().nth(1);
			assert_eq!(
				span.map(|span| span.status),
				Some(SpanStatus::Error {
					error: Some(SpanStatusError {
						name: "provider_error".into(),
						message: "x".repeat(500),
					}),
				})
			);
		});
	}

	#[test]
	fn includes_setup_failures_and_omits_non_scalar_native_reasoning() {
		run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let models = create_models(None);
			let result = telemetry
				.start_span(caller(), move |span| async move {
					Ok(models
						.complete(
							&model(),
							&context(),
							api_options(span, Some(json!({ "effort": "native-secret" }))),
						)
						.await)
				})
				.await
				.unwrap()
				.expect("complete resolves with a message");
			assert_eq!(result.stop_reason, StopReason::Error);
			settled(&telemetry).await;
			let span = telemetry.get_spans().remove(1);
			assert!(!span.attributes.contains_key("reasoning"));
			assert_eq!(
				json_of(&span.events),
				json!([{ "name": "pi.ai.first_chunk", "attributes": {} }])
			);
			assert_eq!(
				span.status,
				SpanStatus::Error {
					error: Some(SpanStatusError {
						name: "provider_error".into(),
						message: "Unknown provider: test".into(),
					}),
				}
			);
			assert!(
				!serde_json::to_string(&span.attributes)
					.unwrap()
					.contains("Unknown provider")
			);
			assert!(!serde_json::to_string(&span).unwrap().contains("native-secret"));
		});
	}

	#[test]
	fn counts_interleaved_tool_deltas_independently_without_reading_arguments() {
		run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let result = message(StopReason::Stop);
			telemetry
				.start_span(caller(), move |span| async move {
					let observer = observe_model_request(&model(), Some(span), None, false);
					let on_event = observer.on_event.clone().unwrap();
					let partial = Rc::new(RefCell::new(result.clone()));
					let tool_call = ToolCall {
						id: "call".into(),
						name: "lookup".into(),
						..ToolCall::default()
					};
					for (content_index, delta) in [(2, "ab"), (3, "123"), (2, "c")] {
						on_event(&AssistantMessageEvent::ToolcallDelta {
							content_index,
							delta: delta.into(),
							partial: partial.clone(),
						});
					}
					for content_index in [3, 2, 4] {
						on_event(&AssistantMessageEvent::ToolcallEnd {
							content_index,
							tool_call: tool_call.clone(),
							partial: partial.clone(),
						});
					}
					observer.on_result.unwrap()(Some(&result));
					Ok(())
				})
				.await
				.unwrap();
			settled(&telemetry).await;
			assert_eq!(
				json_of(&telemetry.get_spans()[1].events[1..].to_vec()),
				json!([
					{ "name": "pi.ai.block", "attributes": { "type": "tool_call", "index": 3.0, "chars": 3.0, "tool": "lookup" } },
					{ "name": "pi.ai.block", "attributes": { "type": "tool_call", "index": 2.0, "chars": 3.0, "tool": "lookup" } },
					{ "name": "pi.ai.block", "attributes": { "type": "tool_call", "index": 4.0, "chars": 0.0, "tool": "lookup" } },
				])
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn supports_a_tracing_only_span_without_metric_capabilities() {
		run(|_| async {
			let adapter = InMemoryTelemetryContext::new();
			let Setup { models, .. } = setup(message(StopReason::Stop));
			adapter
				.start_span(caller(), move |span| async move {
					let telemetry: Rc<dyn TelemetrySpan> = Rc::new(TracingOnlySpan { span });
					Ok(models
						.complete_simple(&model(), &context(), simple_options(telemetry, None))
						.await)
				})
				.await
				.unwrap();
			settled(&adapter).await;
			assert_eq!(adapter.get_spans().len(), 2);
			assert_eq!(adapter.get_metrics(), vec![]);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn defaults_to_no_op_telemetry() {
		run(|_| async {
			let Setup { models, .. } = setup(message(StopReason::Stop));
			assert_eq!(
				models
					.complete_simple(&model(), &context(), None)
					.await
					.map(|result| result.stop_reason),
				Some(StopReason::Stop)
			);
			assert!(AI_TELEMETRY_SCHEMA.spans["pi.ai.request"].start_attributes["provider"].required);
		});
	}

	#[test]
	fn records_low_cardinality_metrics_with_monotonic_durations() {
		run(|host| async move {
			let telemetry = InMemoryTelemetryContext::new();
			let result = message(StopReason::Stop);
			telemetry
				.start_span(caller(), move |span| async move {
					let observer = observe_model_request(&model(), Some(span), None, false);
					host.advance(25.0);
					observer.on_event.unwrap()(&AssistantMessageEvent::Start {
						partial: Rc::new(RefCell::new(result.clone())),
					});
					host.advance(50.0);
					observer.on_result.unwrap()(Some(&result));
					Ok(())
				})
				.await
				.unwrap();
			settled(&telemetry).await;
			let metrics = telemetry.get_metrics();
			let mut expected = vec![
				json!({
					"name": "pi.ai.first_chunk.duration",
					"value": 25.0,
					"attributes": { "provider": "test", "model": "model" },
					"unit": "ms",
				}),
				json!({
					"name": "pi.ai.request.duration",
					"value": 75.0,
					"attributes": { "provider": "test", "model": "model", "stop": "stop" },
					"unit": "ms",
				}),
			];
			for (kind, value) in [
				("input", 11.0),
				("output", 22.0),
				("cache_read", 33.0),
				("cache_write", 44.0),
			] {
				expected.push(json!({
					"name": "pi.ai.tokens",
					"value": value,
					"attributes": { "provider": "test", "model": "model", "kind": kind },
					"unit": "token",
				}));
			}
			expected.push(json!({
				"name": "pi.ai.cost",
				"value": 1.0,
				"attributes": { "provider": "test", "model": "model" },
				"unit": "USD",
			}));
			assert_eq!(
				metrics
					.iter()
					.map(|metric| json!({
						"name": metric.name,
						"value": metric.value,
						"attributes": metric.attributes,
						"unit": metric.unit,
					}))
					.collect::<Vec<_>>(),
				expected
			);
			assert!(metrics[..2].iter().all(|measurement| measurement.context.is_some()));
		});
	}

	fn records_usage_for_a_poll_only_when_final(reason: StopReason) {
		run(|_| async move {
			let telemetry = InMemoryTelemetryContext::new();
			let Setup { models, .. } = setup(message(reason));
			telemetry
				.start_span(caller(), move |span| async move {
					Ok(models.fetch_deferred(&model(), &handle(), deferred_options(span)).await)
				})
				.await
				.unwrap();
			settled(&telemetry).await;
			let counters = telemetry
				.get_metrics()
				.into_iter()
				.filter(|metric| metric.kind == MetricKind::Counter)
				.count();
			assert_eq!(
				counters,
				if matches!(reason, StopReason::Pending | StopReason::Deferred) {
					0
				} else {
					5
				}
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn records_usage_for_a_pending_poll_only_when_final() {
		records_usage_for_a_poll_only_when_final(StopReason::Pending);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn records_usage_for_a_deferred_poll_only_when_final() {
		records_usage_for_a_poll_only_when_final(StopReason::Deferred);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn records_usage_for_a_stop_poll_only_when_final() {
		records_usage_for_a_poll_only_when_final(StopReason::Stop);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn records_usage_for_a_error_poll_only_when_final() {
		records_usage_for_a_poll_only_when_final(StopReason::Error);
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn creates_instruments_once_per_metrics_object_across_root_and_child_requests() {
		run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let spy = Rc::new(SpyMetrics {
				adapter: telemetry.clone(),
				histograms: RefCell::new(Vec::new()),
				counters: RefCell::new(Vec::new()),
			});
			let metrics = spy.clone();
			let spying = move |span: Rc<dyn TelemetrySpan>| -> Rc<dyn TelemetrySpan> {
				Rc::new(SpyingSpan {
					span,
					metrics: metrics.clone(),
				})
			};
			let Setup { models, .. } = setup(message(StopReason::Stop));
			let (first, second, third) = (models.clone(), models.clone(), models);
			let (root, poll, child) = (spying.clone(), spying.clone(), spying);
			telemetry
				.start_span(caller(), move |span| async move {
					Ok(first
						.complete_simple(&model(), &context(), simple_options(root(span), None))
						.await)
				})
				.await
				.unwrap();
			telemetry
				.start_span(caller(), move |span| async move {
					Ok(second
						.fetch_deferred(&model(), &handle(), deferred_options(poll(span)))
						.await)
				})
				.await
				.unwrap();
			telemetry
				.start_span(caller(), move |span| async move {
					Ok(third
						.complete(&model(), &context(), api_options(child(span), None))
						.await)
				})
				.await
				.unwrap();
			settled(&telemetry).await;
			assert_eq!(
				*spy.histograms.borrow(),
				vec!["pi.ai.request.duration", "pi.ai.first_chunk.duration"]
			);
			assert_eq!(*spy.counters.borrow(), vec!["pi.ai.tokens", "pi.ai.cost"]);
			assert_eq!(telemetry.get_metrics().len(), 21);
		});
	}
}

mod lazy_stream_observation {
	use super::*;

	#[test]
	fn uses_one_source_iterator_and_observes_after_push_once_per_terminal_result() {
		run(|_| async {
			let result = message(StopReason::Stop);
			let order = Rc::new(RefCell::new(Vec::<&str>::new()));
			let source = AssistantMessageEventStream::new();
			source.push(AssistantMessageEvent::Done {
				reason: StopReason::Stop,
				message: Rc::new(RefCell::new(result.clone())),
			});
			source.end(Some(result.clone()));
			let outer: Rc<RefCell<Option<AssistantMessageEventStream>>> = Rc::new(RefCell::new(None));
			let (then, event_seen) = oneshot::channel::<()>();
			{
				let (order, outer) = (order.clone(), outer.clone());
				pi_host::spawn_local(async move {
					let _ = event_seen.await;
					let stream = outer
						.borrow()
						.clone()
						.expect("the stream exists before it forwards events");
					stream.result().await;
					order.borrow_mut().push("resolved");
				});
			}
			let then = RefCell::new(Some(then));
			let on_event = {
				let order = order.clone();
				move |_: &AssistantMessageEvent| {
					order.borrow_mut().push("event");
					if let Some(then) = then.borrow_mut().take() {
						let _ = then.send(());
					}
				}
			};
			let on_result = {
				let (order, result) = (order.clone(), result.clone());
				move |received: Option<&AssistantMessage>| {
					assert_eq!(unstamped(received.cloned()), Some(result.clone()));
					order.borrow_mut().push("result");
				}
			};
			let stream = lazy_stream(
				&model(),
				move || async move { Ok(source) },
				Some(LazyStreamObserver {
					on_event: Some(Rc::new(on_event)),
					on_result: Some(Rc::new(on_result)),
				}),
			);
			*outer.borrow_mut() = Some(stream.clone());
			assert_eq!(unstamped(stream.result().await), Some(result));
			wait_for(|| order.borrow().len() == 3).await;
			assert_eq!(*order.borrow(), ["event", "resolved", "result"]);
		});
	}

	fn observed_results() -> (Rc<RefCell<Vec<Option<AssistantMessage>>>>, LazyStreamObserver) {
		let observed = Rc::new(RefCell::new(Vec::new()));
		let record = observed.clone();
		(
			observed,
			LazyStreamObserver {
				on_event: None,
				on_result: Some(Rc::new(move |value: Option<&AssistantMessage>| {
					record.borrow_mut().push(unstamped(value.cloned()));
				})),
			},
		)
	}

	#[test]
	fn reports_a_terminal_result_from_an_iterable_without_a_result_method() {
		run(|_| async {
			let result = message(StopReason::Stop);
			let (observed, observer) = observed_results();
			let source = futures::stream::iter([AssistantMessageEvent::Done {
				reason: StopReason::Stop,
				message: Rc::new(RefCell::new(result.clone())),
			}])
			.boxed_local();
			let stream = lazy_stream(
				&model(),
				move || async move { Ok(LazySource::Events(source)) },
				Some(observer),
			);
			assert_eq!(unstamped(stream.result().await), Some(result.clone()));
			wait_for(|| *observed.borrow() == [Some(result.clone())]).await;
		});
	}

	#[test]
	fn observes_a_result_only_source_once() {
		run(|_| async {
			let result = message(StopReason::Stop);
			let (observed, observer) = observed_results();
			let source = AssistantMessageEventStream::new();
			source.end(Some(result.clone()));
			let stream = lazy_stream(&model(), move || async move { Ok(source) }, Some(observer));
			assert_eq!(unstamped(stream.result().await), Some(result.clone()));
			assert_eq!(*observed.borrow(), [Some(result)]);
		});
	}

	#[test]
	fn reports_setup_failure_events_and_the_result_once() {
		run(|_| async {
			let observed = Rc::new(RefCell::new(Vec::<String>::new()));
			let (events, results) = (observed.clone(), observed.clone());
			let stream = lazy_stream(
				&model(),
				|| async { Err::<AssistantMessageEventStream, _>(PiError::new("setup-secret")) },
				Some(LazyStreamObserver {
					on_event: Some(Rc::new(move |event: &AssistantMessageEvent| {
						events
							.borrow_mut()
							.push(json_of(event)["type"].as_str().unwrap().to_string());
					})),
					on_result: Some(Rc::new(move |result: Option<&AssistantMessage>| {
						results.borrow_mut().push(
							result
								.map(|result| json_of(&result.stop_reason).as_str().unwrap().to_string())
								.unwrap_or_else(|| "missing".into()),
						);
					})),
				}),
			);
			assert_eq!(
				stream.result().await.and_then(|result| result.error_message),
				Some("setup-secret".into())
			);
			assert_eq!(*observed.borrow(), ["error", "error"]);
		});
	}
}
