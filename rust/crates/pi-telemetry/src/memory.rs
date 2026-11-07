//! Port of `packages/telemetry/src/memory.ts`.
//! PORT-STATUS: reviewed
//!
//! The module-level `WeakMap<TelemetryContext, TelemetryCarrier>` is replaced by downcasting: every span and
//! extracted parent that the TS registers in the map is one of this module's types, which carry their carrier,
//! so `context_carrier` finds the carrier for exactly the same objects (across recorder instances, and after
//! settlement). The recorder's `metrics` and `propagation` are the recorder itself, as in TS; it is created
//! behind an `Rc` (`InMemoryTelemetryContext::new`) so it can hand out `Rc` handles to itself.
//!
//! A Rust callback returns a lazy future, so `start_in_memory_span` polls it once before returning, which runs the
//! body up to its first pending await like a TS async callback. A future that is still pending moves into
//! `pi_host::spawn_local`, which settles the span when the callback resolves (the TS `finally`), whether or not
//! the caller polls or keeps the returned future. A future that is ready on the first poll settles before
//! `start_in_memory_span` returns, where TS settles a returned (not thrown) value one microtask later. The eager poll
//! uses a no-op waker, so callbacks that resolve before the executor first runs their tasks settle in spawn order
//! rather than resolution order.
//!
//! JS objects list canonical array-index keys first, in ascending numeric order; `js_key_order` applies that order
//! wherever the TS copies or merges attributes and carriers.
//!
//! `findTraces` parses the cursor into a `serde_json::Value` and applies the TS truthiness, indexing, and
//! comparison coercions to it. An invalid cursor throws a `SyntaxError` whose message is serde_json's, not V8's.

use std::any::Any;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::future::ready;
use std::rc::{Rc, Weak};
use std::task::{Context, Poll};

use futures::channel::oneshot;
use futures::task::noop_waker_ref;
use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string};
use pi_host::{LocalBoxFuture, PiError, PiResult, monotonic_ms, now_ms, spawn_local};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::metrics::{
	HistogramOptions, InstrumentOptions, MetricKind, TelemetryCounter, TelemetryHistogram, TelemetryMetrics,
	TelemetryUpDownCounter,
};
use crate::{
	FindTracesResult, MetricQuery, SpanAttributes, SpanCallback, SpanLink, SpanOptions, SpanStatus, StoredMetric,
	StoredSpan, StoredSpanEvent, StoredSpanLink, StoredTrace, TelemetryCarrier, TelemetryContext, TelemetryPropagation,
	TelemetrySpan, TelemetryStore, TraceQuery,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedTelemetryEvent {
	pub name: String,
	pub attributes: SpanAttributes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedTelemetryLink {
	pub context: TelemetryCarrier,
	pub attributes: SpanAttributes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedTelemetrySpan {
	pub id: f64,
	/// Local parent ID, or null for roots and parents owned by another recorder.
	pub parent_id: Option<f64>,
	pub context: TelemetryCarrier,
	pub parent_context: Option<TelemetryCarrier>,
	pub links: Vec<RecordedTelemetryLink>,
	pub name: String,
	pub attributes: SpanAttributes,
	pub events: Vec<RecordedTelemetryEvent>,
	pub status: SpanStatus,
	pub settled: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub end_sequence: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedTelemetryMeasurement {
	pub kind: MetricKind,
	pub name: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub unit: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub boundaries: Option<Vec<f64>>,
	pub value: f64,
	pub attributes: SpanAttributes,
	pub context: Option<TelemetryCarrier>,
}

struct MutableRecordedTelemetryEvent {
	time_ms: f64,
	name: String,
	attributes: SpanAttributes,
}

struct MutableRecordedTelemetrySpan {
	started_ms: f64,
	clock: f64,
	ended_ms: Option<f64>,
	id: f64,
	parent_id: Option<f64>,
	context: TelemetryCarrier,
	parent_context: Option<TelemetryCarrier>,
	links: Vec<RecordedTelemetryLink>,
	name: String,
	attributes: SpanAttributes,
	events: Vec<MutableRecordedTelemetryEvent>,
	status: SpanStatus,
	explicit_status: bool,
	settled: bool,
	end_sequence: Option<f64>,
}

type SharedRecordedSpan = Rc<RefCell<MutableRecordedTelemetrySpan>>;

struct InMemoryParent {
	context: TelemetryCarrier,
	local_id: Option<f64>,
	span: Option<SharedRecordedSpan>,
}

fn carrier_value(carrier: &TelemetryCarrier, key: &str) -> String {
	carrier.get(key).cloned().unwrap_or_else(|| "undefined".to_string())
}

fn carrier_id(carrier: &TelemetryCarrier) -> String {
	format!(
		"{}:{}",
		carrier_value(carrier, "pi-memory-scope"),
		carrier_value(carrier, "pi-memory-span")
	)
}

/// `contextCarriers.get(context)`: identity survives settlement without exposing backend IDs on the generic
/// context contract.
fn context_carrier(context: &dyn TelemetryContext) -> Option<&TelemetryCarrier> {
	let any: &dyn Any = context;
	if let Some(span) = any.downcast_ref::<InMemorySpan>() {
		return Some(&span.context);
	}
	any.downcast_ref::<InMemoryExtractedParent>()
		.map(|parent| &parent.context)
}

struct MeasurementEntry {
	interval_ms: f64,
	measurement: RecordedTelemetryMeasurement,
}

struct InMemoryTelemetryState {
	spans: Vec<SharedRecordedSpan>,
	measurements: Vec<MeasurementEntry>,
	next_span_id: f64,
	next_end_sequence: f64,
}

/// Orders keys like a JS object: canonical array-index keys first in ascending numeric order, then the other keys
/// in insertion order.
fn js_key_order<V>(mut object: IndexMap<String, V>) -> IndexMap<String, V> {
	object.sort_by(|left, _, right, _| match (array_index(left), array_index(right)) {
		(Some(left), Some(right)) => left.cmp(&right),
		(Some(_), None) => Ordering::Less,
		(None, Some(_)) => Ordering::Greater,
		(None, None) => Ordering::Equal,
	});
	object
}

/// The value of a canonical array-index key: a decimal integer from 0 to 2^32 - 2 without leading zeros.
fn array_index(key: &str) -> Option<u64> {
	if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
		return None;
	}
	if !key.bytes().all(|byte| byte.is_ascii_digit()) {
		return None;
	}
	key.parse::<u64>().ok().filter(|index| *index <= 4_294_967_294)
}

fn copy_attributes(attributes: Option<&SpanAttributes>) -> SpanAttributes {
	js_key_order(
		attributes
			.into_iter()
			.flatten()
			.filter(|(_, value)| value.is_some())
			.map(|(name, value)| (name.clone(), value.clone()))
			.collect(),
	)
}

fn copy_link(link: &SpanLink) -> Option<RecordedTelemetryLink> {
	context_carrier(link.context.as_ref()).map(|context| RecordedTelemetryLink {
		context: js_key_order(context.clone()),
		attributes: copy_attributes(link.attributes.as_ref()),
	})
}

fn create_span(
	telemetry: &InMemoryTelemetryContext,
	parent: Option<&InMemoryParent>,
	options: &SpanOptions,
) -> MutableRecordedTelemetrySpan {
	let name = options.name.clone();
	let attributes = copy_attributes(options.attributes.as_ref());
	let mut links = Vec::new();
	for link in options.links.iter().flatten() {
		if let Some(copy) = copy_link(link) {
			links.push(copy);
		}
	}
	let id = {
		let mut state = telemetry.state.borrow_mut();
		let id = state.next_span_id;
		state.next_span_id += 1.0;
		id
	};
	let started_ms = now_ms();
	let clock = monotonic_ms();
	let trace = parent
		.and_then(|parent| parent.context.get("pi-memory-trace").cloned())
		.unwrap_or_else(|| format!("{}:{}", telemetry.scope, number_to_string(id)));
	let mut context = TelemetryCarrier::new();
	context.insert("pi-memory-scope".to_string(), telemetry.scope.clone());
	context.insert("pi-memory-span".to_string(), number_to_string(id));
	context.insert("pi-memory-trace".to_string(), trace);
	MutableRecordedTelemetrySpan {
		started_ms,
		clock,
		ended_ms: None,
		id,
		parent_id: parent.and_then(|parent| parent.local_id),
		context,
		parent_context: parent.map(|parent| js_key_order(parent.context.clone())),
		links,
		name,
		attributes,
		events: Vec::new(),
		status: SpanStatus::Ok,
		explicit_status: false,
		settled: false,
		end_sequence: None,
	}
}

/// A span started under a settled parent: it records nothing but keeps the recorder's capabilities.
struct InertSpan {
	telemetry: Rc<InMemoryTelemetryContext>,
}

impl TelemetryContext for InertSpan {
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		Some(self.telemetry.clone())
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		Some(self.telemetry.clone())
	}

	fn start_span_erased(
		&self,
		options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
		start_inert_span(self.telemetry.clone(), options, callback)
	}
}

impl TelemetrySpan for InertSpan {
	fn add_link(&self, _link: &SpanLink) {}

	fn add_event(&self, _name: &str, _attributes: Option<&SpanAttributes>) {}

	fn set_attributes(&self, _attributes: &SpanAttributes) {}

	fn set_status(&self, _status: &SpanStatus) {}
}

fn start_inert_span(
	telemetry: Rc<InMemoryTelemetryContext>,
	_options: SpanOptions,
	callback: SpanCallback,
) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
	callback(Rc::new(InertSpan { telemetry }))
}

/// A live or settled recorded span.
struct InMemorySpan {
	telemetry: Rc<InMemoryTelemetryContext>,
	recorded: SharedRecordedSpan,
	context: TelemetryCarrier,
}

impl TelemetryContext for InMemorySpan {
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		Some(self.telemetry.clone())
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		Some(self.telemetry.clone())
	}

	fn start_span_erased(
		&self,
		options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
		let local_id = self.recorded.borrow().id;
		start_in_memory_span(
			self.telemetry.clone(),
			Some(InMemoryParent {
				context: self.context.clone(),
				local_id: Some(local_id),
				span: Some(self.recorded.clone()),
			}),
			options,
			callback,
		)
	}
}

impl TelemetrySpan for InMemorySpan {
	fn add_link(&self, link: &SpanLink) {
		if self.recorded.borrow().settled {
			return;
		}
		if let Some(copy) = copy_link(link) {
			self.recorded.borrow_mut().links.push(copy);
		}
	}

	fn add_event(&self, name: &str, attributes: Option<&SpanAttributes>) {
		if self.recorded.borrow().settled {
			return;
		}
		let time_ms = monotonic_ms() - self.recorded.borrow().clock;
		self.recorded.borrow_mut().events.push(MutableRecordedTelemetryEvent {
			name: name.to_string(),
			time_ms,
			attributes: copy_attributes(attributes),
		});
	}

	fn set_attributes(&self, attributes: &SpanAttributes) {
		let mut recorded = self.recorded.borrow_mut();
		if recorded.settled {
			return;
		}
		let mut merged = std::mem::take(&mut recorded.attributes);
		merged.extend(copy_attributes(Some(attributes)));
		recorded.attributes = js_key_order(merged);
	}

	fn set_status(&self, status: &SpanStatus) {
		let mut recorded = self.recorded.borrow_mut();
		if recorded.settled {
			return;
		}
		recorded.status = status.clone();
		recorded.explicit_status = true;
	}
}

/// A parent context returned by `extract` for a recognized carrier.
struct InMemoryExtractedParent {
	telemetry: Rc<InMemoryTelemetryContext>,
	context: TelemetryCarrier,
	local_id: Option<f64>,
}

impl TelemetryContext for InMemoryExtractedParent {
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		Some(self.telemetry.clone())
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		Some(self.telemetry.clone())
	}

	fn start_span_erased(
		&self,
		options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
		start_in_memory_span(
			self.telemetry.clone(),
			Some(InMemoryParent {
				context: self.context.clone(),
				local_id: self.local_id,
				span: None,
			}),
			options,
			callback,
		)
	}
}

fn start_in_memory_span(
	telemetry: Rc<InMemoryTelemetryContext>,
	parent: Option<InMemoryParent>,
	options: SpanOptions,
	callback: SpanCallback,
) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
	if parent
		.as_ref()
		.and_then(|parent| parent.span.as_ref())
		.is_some_and(|span| span.borrow().settled)
	{
		return start_inert_span(telemetry, options, callback);
	}

	let recorded = Rc::new(RefCell::new(create_span(&telemetry, parent.as_ref(), &options)));
	telemetry.state.borrow_mut().spans.push(recorded.clone());

	let context = recorded.borrow().context.clone();
	let span = Rc::new(InMemorySpan {
		telemetry: telemetry.clone(),
		recorded: recorded.clone(),
		context,
	});

	let mut future = callback(span);
	if let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(noop_waker_ref())) {
		settle_span(&telemetry, &recorded, &result);
		return Box::pin(ready(result));
	}
	let (sender, receiver) = oneshot::channel();
	spawn_local(async move {
		let result = future.await;
		settle_span(&telemetry, &recorded, &result);
		let _ = sender.send(result);
	});
	Box::pin(async move {
		receiver
			.await
			.expect("the spawned span task always sends the callback's result")
	})
}

/// The `catch` and `finally` of `startInMemorySpan`: records the rejection status and settles the span.
fn settle_span(telemetry: &InMemoryTelemetryContext, recorded: &SharedRecordedSpan, result: &PiResult<Box<dyn Any>>) {
	let mut span = recorded.borrow_mut();
	if let Err(error) = result
		&& !span.explicit_status
	{
		span.status = SpanStatus::Error {
			error: (!error.name.is_empty()).then(|| crate::SpanStatusError {
				name: error.name.clone(),
				message: error.message.clone(),
			}),
		};
	}
	span.settled = true;
	span.ended_ms = Some(span.started_ms + monotonic_ms() - span.clock);
	let mut state = telemetry.state.borrow_mut();
	span.end_sequence = Some(state.next_end_sequence);
	state.next_end_sequence += 1.0;
}

struct MeasurementRecorder {
	telemetry: Rc<InMemoryTelemetryContext>,
	kind: MetricKind,
	name: String,
	description: Option<String>,
	unit: Option<String>,
	boundaries: Option<Vec<f64>>,
}

impl MeasurementRecorder {
	fn new(
		telemetry: Rc<InMemoryTelemetryContext>,
		kind: MetricKind,
		name: &str,
		options: Option<&InstrumentOptions>,
		boundaries: Option<&[f64]>,
	) -> Self {
		Self {
			telemetry,
			kind,
			name: name.to_string(),
			description: options.and_then(|options| options.description.clone()),
			unit: options.and_then(|options| options.unit.clone()),
			boundaries: if kind == MetricKind::Histogram {
				boundaries.map(<[f64]>::to_vec)
			} else {
				None
			},
		}
	}

	fn record(&self, value: f64, attributes: Option<&SpanAttributes>, context: Option<&dyn TelemetryContext>) {
		let carrier = context.and_then(context_carrier);
		let interval_ms = (now_ms() / 10_000.0).floor() * 10_000.0;
		let measurement = RecordedTelemetryMeasurement {
			kind: self.kind,
			name: self.name.clone(),
			description: self.description.clone(),
			unit: self.unit.clone(),
			boundaries: self.boundaries.clone(),
			value,
			attributes: copy_attributes(attributes),
			context: carrier.cloned().map(js_key_order),
		};
		self.telemetry.state.borrow_mut().measurements.push(MeasurementEntry {
			interval_ms,
			measurement,
		});
	}
}

impl TelemetryCounter for MeasurementRecorder {
	fn add(&self, value: f64, attributes: Option<&SpanAttributes>) {
		self.record(value, attributes, None);
	}
}

impl TelemetryUpDownCounter for MeasurementRecorder {
	fn add(&self, value: f64, attributes: Option<&SpanAttributes>) {
		self.record(value, attributes, None);
	}
}

impl TelemetryHistogram for MeasurementRecorder {
	fn record(&self, value: f64, attributes: Option<&SpanAttributes>, context: Option<&dyn TelemetryContext>) {
		MeasurementRecorder::record(self, value, attributes, context);
	}
}

/// Backend-neutral reference implementation that records spans in process memory.
/// Create a fresh instance to isolate tests or independent recording scopes.
pub struct InMemoryTelemetryContext {
	this: Weak<InMemoryTelemetryContext>,
	scope: String,
	state: RefCell<InMemoryTelemetryState>,
}

impl InMemoryTelemetryContext {
	/// `new InMemoryTelemetryContext()`.
	pub fn new() -> Rc<Self> {
		Rc::new_cyclic(|this| Self {
			this: this.clone(),
			scope: random_uuid(),
			state: RefCell::new(InMemoryTelemetryState {
				spans: Vec::new(),
				measurements: Vec::new(),
				next_span_id: 1.0,
				next_end_sequence: 1.0,
			}),
		})
	}

	fn this(&self) -> Rc<Self> {
		self.this
			.upgrade()
			.expect("InMemoryTelemetryContext is only reachable through the Rc that new() returns")
	}

	fn stored_spans(&self) -> Vec<StoredSpan> {
		let mut spans: Vec<StoredSpan> = self
			.state
			.borrow()
			.spans
			.iter()
			.map(|span| span.borrow())
			.filter(|span| span.settled)
			.map(|span| StoredSpan {
				id: carrier_id(&span.context),
				trace_id: carrier_value(&span.context, "pi-memory-trace"),
				parent_id: span.parent_context.as_ref().map(carrier_id),
				name: span.name.clone(),
				started_ms: span.started_ms,
				ended_ms: span.ended_ms.expect("settled spans have an end time"),
				attributes: copy_attributes(Some(&span.attributes)),
				events: span
					.events
					.iter()
					.map(|event| StoredSpanEvent {
						name: event.name.clone(),
						time_ms: event.time_ms,
						attributes: copy_attributes(Some(&event.attributes)),
					})
					.collect(),
				links: span
					.links
					.iter()
					.map(|link| StoredSpanLink {
						trace_id: carrier_value(&link.context, "pi-memory-trace"),
						span_id: carrier_id(&link.context),
						attributes: Some(copy_attributes(Some(&link.attributes))),
					})
					.collect(),
				status: span.status.clone(),
			})
			.collect();
		spans.sort_by(|a, b| number_order(a.started_ms - b.started_ms).then_with(|| utf16_cmp(&a.id, &b.id)));
		spans
	}

	/// `getTrace(traceId)`.
	pub fn get_trace_now(&self, trace_id: &str) -> Option<StoredTrace> {
		let spans: Vec<StoredSpan> = self
			.stored_spans()
			.into_iter()
			.filter(|span| span.trace_id == trace_id)
			.collect();
		if spans.is_empty() {
			None
		} else {
			Some(StoredTrace {
				trace_id: trace_id.to_string(),
				spans,
			})
		}
	}

	/// `findTraces(query = {})`.
	pub fn find_traces_now(&self, query: Option<TraceQuery>) -> PiResult<FindTracesResult> {
		let query = query.unwrap_or_default();
		let limit = query.limit.unwrap_or(20.0);
		let after = match &query.cursor {
			None => None,
			Some(cursor) => Some(serde_json::from_str::<Value>(cursor)?),
		};
		let mut grouped: IndexMap<String, StoredTrace> = IndexMap::new();
		for span in self.stored_spans() {
			grouped
				.entry(span.trace_id.clone())
				.or_insert_with(|| StoredTrace {
					trace_id: span.trace_id.clone(),
					spans: Vec::new(),
				})
				.spans
				.push(span);
		}
		let mut matches: Vec<StoredTrace> = grouped
			.into_values()
			.filter(|trace| {
				let first = &trace.spans[0];
				query.name.as_ref().is_none_or(|name| &first.name == name)
					&& query.start_min.is_none_or(|start_min| first.started_ms >= start_min)
					&& query.start_max.is_none_or(|start_max| first.started_ms <= start_max)
					&& after.as_ref().is_none_or(|after| {
						let started_ms = js_index(after, 0);
						!js_truthy(after)
							|| first.started_ms < js_primitive(started_ms.as_ref()).to_number()
							|| (started_ms.as_ref().and_then(Value::as_f64) == Some(first.started_ms)
								&& match js_primitive(js_index(after, 1).as_ref()) {
									JsPrimitive::String(trace_id) => {
										utf16_cmp(&trace.trace_id, &trace_id) == Ordering::Greater
									}
									JsPrimitive::Number(trace_id) => js_number(&trace.trace_id) > trace_id,
								})
					}) && (query.errors != Some(true)
					|| trace
						.spans
						.iter()
						.any(|span| matches!(span.status, SpanStatus::Error { .. })))
					&& query.attributes.as_ref().is_none_or(|attributes| {
						trace.spans.iter().any(|span| {
							attributes.iter().all(|(key, value)| {
								span.attributes
									.get(key)
									.and_then(Option::as_ref)
									.is_some_and(|actual| actual.strict_equals(value))
							})
						})
					})
			})
			.collect();
		matches.sort_by(|a, b| {
			number_order(b.spans[0].started_ms - a.spans[0].started_ms)
				.then_with(|| utf16_cmp(&a.trace_id, &b.trace_id))
		});
		let has_more = (matches.len() as f64) > limit;
		let end = slice_end(limit, matches.len());
		matches.truncate(end);
		let traces = matches;
		let cursor = if has_more {
			let last = traces
				.last()
				.ok_or_else(|| PiError::type_error("Cannot read properties of undefined (reading 'spans')"))?;
			Some(json_stringify(&(last.spans[0].started_ms, &last.trace_id)))
		} else {
			None
		};
		Ok(FindTracesResult { traces, cursor })
	}

	/// Read raw measurements as count-one rows, without changing getMetrics() snapshots.
	pub fn find_metrics_now(&self, query: &MetricQuery) -> Vec<StoredMetric> {
		self.state
			.borrow()
			.measurements
			.iter()
			.filter(
				|MeasurementEntry {
				     interval_ms,
				     measurement,
				 }| {
					measurement.name == query.name
						&& query.start_min.is_none_or(|start_min| *interval_ms >= start_min)
						&& query.start_max.is_none_or(|start_max| *interval_ms <= start_max)
						&& query.attributes.iter().flatten().all(|(key, value)| {
							measurement
								.attributes
								.get(key)
								.and_then(Option::as_ref)
								.is_some_and(|actual| actual.strict_equals(value))
						})
				},
			)
			.map(
				|MeasurementEntry {
				     interval_ms,
				     measurement,
				 }| StoredMetric {
					interval_ms: *interval_ms,
					name: measurement.name.clone(),
					kind: measurement.kind,
					unit: measurement.unit.clone(),
					attributes: copy_attributes(Some(&measurement.attributes)),
					count: 1.0,
					sum: measurement.value,
					min: measurement.value,
					max: measurement.value,
				},
			)
			.collect()
	}

	/// Returns detached measurements in recording order, without aggregation.
	pub fn get_metrics(&self) -> Vec<RecordedTelemetryMeasurement> {
		self.state
			.borrow()
			.measurements
			.iter()
			.map(|entry| entry.measurement.clone())
			.collect()
	}

	/// Returns detached snapshots in span-start order.
	pub fn get_spans(&self) -> Vec<RecordedTelemetrySpan> {
		self.state
			.borrow()
			.spans
			.iter()
			.map(|span| {
				let span = span.borrow();
				RecordedTelemetrySpan {
					id: span.id,
					parent_id: span.parent_id,
					context: span.context.clone(),
					parent_context: span.parent_context.clone(),
					links: span.links.clone(),
					name: span.name.clone(),
					attributes: copy_attributes(Some(&span.attributes)),
					events: span
						.events
						.iter()
						.map(|event| RecordedTelemetryEvent {
							name: event.name.clone(),
							attributes: copy_attributes(Some(&event.attributes)),
						})
						.collect(),
					status: span.status.clone(),
					settled: span.settled,
					end_sequence: span.end_sequence,
				}
			})
			.collect()
	}
}

impl TelemetryContext for InMemoryTelemetryContext {
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		Some(self.this())
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		Some(self.this())
	}

	fn start_span_erased(
		&self,
		options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
		start_in_memory_span(self.this(), None, options, callback)
	}
}

impl TelemetryPropagation for InMemoryTelemetryContext {
	/// Returns a detached carrier for a known span or extracted parent, including settled spans.
	fn inject(&self, context: &dyn TelemetryContext) -> Option<TelemetryCarrier> {
		context_carrier(context).cloned().map(js_key_order)
	}

	/// Missing or non-string carrier identities fall back to this recorder's root context.
	fn extract(&self, carrier: Option<&TelemetryCarrier>) -> Rc<dyn TelemetryContext> {
		let scope = carrier.and_then(|carrier| carrier.get("pi-memory-scope"));
		let span = carrier.and_then(|carrier| carrier.get("pi-memory-span"));
		let trace_id = carrier.and_then(|carrier| carrier.get("pi-memory-trace"));
		let (Some(scope), Some(span), Some(trace_id)) = (scope, span, trace_id) else {
			return self.this();
		};
		let mut context = TelemetryCarrier::new();
		context.insert("pi-memory-scope".to_string(), scope.clone());
		context.insert("pi-memory-span".to_string(), span.clone());
		context.insert("pi-memory-trace".to_string(), trace_id.clone());
		let local_id = if *scope == self.scope {
			Some(js_number(span))
		} else {
			None
		};
		Rc::new(InMemoryExtractedParent {
			telemetry: self.this(),
			context,
			local_id,
		})
	}
}

impl TelemetryMetrics for InMemoryTelemetryContext {
	fn counter(&self, name: &str, options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryCounter> {
		Rc::new(MeasurementRecorder::new(
			self.this(),
			MetricKind::Counter,
			name,
			options,
			None,
		))
	}

	fn up_down_counter(&self, name: &str, options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryUpDownCounter> {
		Rc::new(MeasurementRecorder::new(
			self.this(),
			MetricKind::UpDownCounter,
			name,
			options,
			None,
		))
	}

	fn histogram(&self, name: &str, options: Option<&HistogramOptions>) -> Rc<dyn TelemetryHistogram> {
		Rc::new(MeasurementRecorder::new(
			self.this(),
			MetricKind::Histogram,
			name,
			options.map(|options| &options.base),
			options.and_then(|options| options.boundaries.as_deref()),
		))
	}
}

impl TelemetryStore for InMemoryTelemetryContext {
	fn find_traces(&self, query: Option<TraceQuery>) -> LocalBoxFuture<'static, PiResult<FindTracesResult>> {
		let result = self.find_traces_now(query);
		Box::pin(async move { result })
	}

	fn get_trace(&self, trace_id: &str) -> LocalBoxFuture<'static, PiResult<Option<StoredTrace>>> {
		let result = self.get_trace_now(trace_id);
		Box::pin(async move { Ok(result) })
	}

	fn find_metrics(&self, query: MetricQuery) -> LocalBoxFuture<'static, PiResult<Vec<StoredMetric>>> {
		let result = self.find_metrics_now(&query);
		Box::pin(async move { Ok(result) })
	}
}

/// A JS sort comparator result (`a - b`) as an ordering; `NaN` compares equal, like `Array.prototype.sort`.
fn number_order(difference: f64) -> Ordering {
	difference.partial_cmp(&0.0).unwrap_or(Ordering::Equal)
}

/// JavaScript `<` / `>` on strings: lexicographic over UTF-16 code units.
fn utf16_cmp(left: &str, right: &str) -> Ordering {
	left.encode_utf16().cmp(right.encode_utf16())
}

/// The exclusive end of `array.slice(0, end)` for a JS number `end`.
fn slice_end(end: f64, len: usize) -> usize {
	if end.is_nan() {
		return 0;
	}
	let end = end.trunc();
	if end < 0.0 {
		(len as f64 + end).max(0.0) as usize
	} else {
		end.min(len as f64) as usize
	}
}

/// JS truthiness of a parsed JSON value.
fn js_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(value) => *value,
		Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
		Value::String(value) => !value.is_empty(),
		Value::Array(_) | Value::Object(_) => true,
	}
}

/// `value[index]` on a parsed JSON value; `None` is `undefined`.
fn js_index(value: &Value, index: usize) -> Option<Value> {
	match value {
		Value::Array(items) => items.get(index).cloned(),
		Value::Object(entries) => entries.get(&index.to_string()).cloned(),
		Value::String(text) => text
			.encode_utf16()
			.nth(index)
			.map(|unit| Value::String(String::from_utf16_lossy(&[unit]))),
		Value::Null | Value::Bool(_) | Value::Number(_) => None,
	}
}

/// An operand of a JS relational comparison after `ToPrimitive`. `undefined`, `null`, and booleans are kept
/// as the numbers the comparison converts them to, because they never compare as strings.
enum JsPrimitive {
	Number(f64),
	String(String),
}

impl JsPrimitive {
	/// `ToNumber`.
	fn to_number(&self) -> f64 {
		match self {
			JsPrimitive::Number(value) => *value,
			JsPrimitive::String(value) => js_number(value),
		}
	}
}

/// `ToPrimitive(value, number)` for a parsed JSON value or `undefined` (`None`).
fn js_primitive(value: Option<&Value>) -> JsPrimitive {
	match value {
		None => JsPrimitive::Number(f64::NAN),
		Some(Value::Null) => JsPrimitive::Number(0.0),
		Some(Value::Bool(value)) => JsPrimitive::Number(if *value { 1.0 } else { 0.0 }),
		Some(Value::Number(value)) => JsPrimitive::Number(value.as_f64().unwrap_or(f64::NAN)),
		Some(value) => JsPrimitive::String(js_string(value)),
	}
}

/// `String(value)` for a parsed JSON value; arrays join their elements with `,`, printing `null` elements as empty
/// strings.
fn js_string(value: &Value) -> String {
	match value {
		Value::Null => "null".to_string(),
		Value::Bool(value) => value.to_string(),
		Value::Number(value) => number_to_string(value.as_f64().unwrap_or(f64::NAN)),
		Value::String(value) => value.clone(),
		Value::Array(items) => items
			.iter()
			.map(|item| match item {
				Value::Null => String::new(),
				item => js_string(item),
			})
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".to_string(),
	}
}

/// `Number(string)`.
fn js_number(value: &str) -> f64 {
	let trimmed = value.trim_matches(|ch: char| (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}');
	if trimmed.is_empty() {
		return 0.0;
	}
	for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
		if let Some(digits) = trimmed.strip_prefix(prefix) {
			if digits.is_empty() {
				return f64::NAN;
			}
			let mut result = 0.0;
			for ch in digits.chars() {
				match ch.to_digit(radix) {
					Some(digit) => result = result * f64::from(radix) + f64::from(digit),
					None => return f64::NAN,
				}
			}
			return result;
		}
	}
	match trimmed {
		"Infinity" | "+Infinity" => return f64::INFINITY,
		"-Infinity" => return f64::NEG_INFINITY,
		_ => {}
	}
	let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
	let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
		Some(index) => (&unsigned[..index], Some(&unsigned[index + 1..])),
		None => (unsigned, None),
	};
	let (integer, fraction) = match mantissa.split_once('.') {
		Some((integer, fraction)) => (integer, Some(fraction)),
		None => (mantissa, None),
	};
	let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
	let mantissa_valid = digits(integer)
		&& fraction.is_none_or(digits)
		&& (!integer.is_empty() || fraction.is_some_and(|fraction| !fraction.is_empty()));
	let exponent_valid = exponent.is_none_or(|exponent| {
		let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
		!exponent.is_empty() && digits(exponent)
	});
	if !mantissa_valid || !exponent_valid {
		return f64::NAN;
	}
	trimmed.parse().unwrap_or(f64::NAN)
}

/// `crypto.randomUUID()`: a random version 4 UUID in lowercase hex.
fn random_uuid() -> String {
	let mut bytes: [u8; 16] = rand::random();
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
	format!(
		"{}-{}-{}-{}-{}",
		&hex[0..8],
		&hex[8..12],
		&hex[12..16],
		&hex[16..20],
		&hex[20..32]
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::render::render_trace;
	use crate::{AttributeValue, SpanStatusError, TelemetryContextExt};

	fn options(name: &str) -> SpanOptions {
		SpanOptions {
			name: name.to_string(),
			..SpanOptions::default()
		}
	}

	fn find(spans: &[RecordedTelemetrySpan], name: &str) -> RecordedTelemetrySpan {
		spans
			.iter()
			.find(|span| span.name == name)
			.expect("span was recorded")
			.clone()
	}

	#[test]
	fn settles_spans_whose_future_is_dropped() {
		let context = pi_host::testing::run(|_| async {
			let context = InMemoryTelemetryContext::new();
			drop(context.start_span(options("sync"), |_| ready(Ok(()))));
			let (release, gate) = oneshot::channel::<()>();
			drop(context.start_span(options("async"), move |_| async move {
				let _ = gate.await;
				Ok(())
			}));
			let _ = release.send(());
			context
		});
		let spans = context.get_spans();
		assert_eq!(
			spans
				.iter()
				.map(|span| (span.name.as_str(), span.settled, span.end_sequence))
				.collect::<Vec<_>>(),
			vec![("sync", true, Some(1.0)), ("async", true, Some(2.0))]
		);
		assert_eq!(context.find_traces_now(None).unwrap().traces.len(), 2);
	}

	#[test]
	fn assigns_end_sequences_in_resolution_order() {
		pi_host::testing::run(|_| async {
			let context = InMemoryTelemetryContext::new();
			let (release_a, gate_a) = oneshot::channel::<()>();
			let (release_b, gate_b) = oneshot::channel::<()>();
			let a = context.start_span(options("a"), move |_| async move {
				let _ = gate_a.await;
				Ok(())
			});
			let b = context.start_span(options("b"), move |_| async move {
				let _ = gate_b.await;
				Ok(())
			});
			let mut yielded = false;
			futures::future::poll_fn(|cx| {
				if yielded {
					return Poll::Ready(());
				}
				yielded = true;
				cx.waker().wake_by_ref();
				Poll::Pending
			})
			.await;
			let _ = release_b.send(());
			let _ = release_a.send(());
			let (a, b) = futures::future::join(a, b).await;
			a.unwrap();
			b.unwrap();
			let spans = context.get_spans();
			assert!(find(&spans, "b").end_sequence < find(&spans, "a").end_sequence);
		});
	}

	#[test]
	fn settles_synchronous_rejections_before_the_result_is_awaited() {
		pi_host::testing::run(|_| async {
			let context = InMemoryTelemetryContext::new();
			let result = context.start_span(options("sync-error"), |_| ready(Err::<(), _>(PiError::new("sync"))));
			let span = find(&context.get_spans(), "sync-error");
			assert!(span.settled);
			assert_eq!(span.end_sequence, Some(1.0));
			assert_eq!(
				span.status,
				SpanStatus::Error {
					error: Some(SpanStatusError {
						name: "Error".to_string(),
						message: "sync".to_string(),
					}),
				}
			);
			assert_eq!(result.await, Err(PiError::new("sync")));
		});
	}

	#[test]
	fn records_non_error_rejections_without_error_details() {
		pi_host::testing::run(|_| async {
			let context = InMemoryTelemetryContext::new();
			let thrown = PiError::named("", "undefined");
			let result = context
				.start_span(options("undefined-error"), move |_| async move { Err::<(), _>(thrown) })
				.await;
			assert_eq!(result, Err(PiError::named("", "undefined")));
			assert_eq!(
				find(&context.get_spans(), "undefined-error").status,
				SpanStatus::Error { error: None }
			);
		});
	}

	#[test]
	fn orders_integer_like_attribute_keys_like_javascript_objects() {
		pi_host::testing::run(|_| async {
			let context = InMemoryTelemetryContext::new();
			let start: SpanAttributes = [("b", AttributeValue::from("x")), ("1", AttributeValue::from("y"))]
				.into_iter()
				.map(|(key, value)| (key.to_string(), Some(value)))
				.collect();
			context
				.start_span(
					SpanOptions {
						name: "k".to_string(),
						attributes: Some(start),
						links: None,
					},
					|span| {
						let merged: SpanAttributes =
							[("a", AttributeValue::from(1.0)), ("0", AttributeValue::from(2.0))]
								.into_iter()
								.map(|(key, value)| (key.to_string(), Some(value)))
								.collect();
						span.set_attributes(&merged);
						let event: SpanAttributes = [
							("b", AttributeValue::from(1.0)),
							("10", AttributeValue::from(2.0)),
							("2", AttributeValue::from(3.0)),
							("01", AttributeValue::from(4.0)),
							("4294967295", AttributeValue::from(5.0)),
						]
						.into_iter()
						.map(|(key, value)| (key.to_string(), Some(value)))
						.collect();
						span.add_event("e", Some(&event));
						ready(Ok(()))
					},
				)
				.await
				.unwrap();
			let span = find(&context.get_spans(), "k");
			assert_eq!(span.attributes.keys().collect::<Vec<_>>(), vec!["0", "1", "b", "a"]);
			assert_eq!(
				json_stringify(&span.events[0].attributes),
				r#"{"2":3,"10":2,"b":1,"01":4,"4294967295":5}"#
			);
			let trace = context.find_traces_now(None).unwrap().traces.remove(0);
			assert_eq!(
				render_trace(&trace.spans).lines().next(),
				Some(r#"k 0=2 1="y" b="x" a=1 (0ms)"#)
			);
		});
	}

	#[test]
	fn applies_javascript_semantics_to_parsed_cursors() {
		pi_host::testing::run(|_| async {
			let context = InMemoryTelemetryContext::new();
			context.start_span(options("only"), |_| ready(Ok(()))).await.unwrap();
			let traces = |cursor: &str| {
				context
					.find_traces_now(Some(TraceQuery {
						cursor: Some(cursor.to_string()),
						..TraceQuery::default()
					}))
					.map(|result| result.traces.len())
			};
			let started = number_to_string(context.stored_spans()[0].started_ms);
			assert_eq!(traces("null"), Ok(1));
			assert_eq!(traces("0"), Ok(1));
			assert_eq!(traces("\"\""), Ok(1));
			assert_eq!(traces("{}"), Ok(0));
			assert_eq!(traces("5"), Ok(0));
			assert_eq!(traces("[1]"), Ok(0));
			assert_eq!(traces(r#"[1,"a",3]"#), Ok(0));
			assert_eq!(traces(r#"["1700000000001","x"]"#), Ok(1));
			assert_eq!(traces(r#"[["1700000000001"],"x"]"#), Ok(1));
			assert_eq!(traces(&format!(r#"[{started},""]"#)), Ok(1));
			assert_eq!(traces(&format!(r#"[{started},"~"]"#)), Ok(0));
			assert_eq!(traces(&format!("[{started},null]")), Ok(0));
			assert_eq!(
				traces("abc").map_err(|error| error.name),
				Err("SyntaxError".to_string())
			);
		});
	}

	#[test]
	fn number_conversion_matches_javascript() {
		assert_eq!(js_number("12"), 12.0);
		assert_eq!(js_number(" 0x1f "), 31.0);
		assert_eq!(js_number(""), 0.0);
		assert_eq!(js_number("1e3"), 1000.0);
		assert_eq!(js_number(".5"), 0.5);
		assert_eq!(js_number("-Infinity"), f64::NEG_INFINITY);
		assert!(js_number("inf").is_nan());
		assert!(js_number("external-span").is_nan());
		assert!(js_number("1.").eq(&1.0));
		assert!(js_number(".").is_nan());
	}

	#[test]
	fn uuid_has_version_four_layout() {
		let uuid = random_uuid();
		assert_eq!(uuid.len(), 36);
		assert_eq!(&uuid[14..15], "4");
	}
}
