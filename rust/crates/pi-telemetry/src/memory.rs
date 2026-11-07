//! Port of `packages/telemetry/src/memory.ts`.
//! PORT-STATUS: ported
//!
//! The module-level `WeakMap<TelemetryContext, TelemetryCarrier>` is replaced by downcasting: every span and
//! extracted parent that the TS registers in the map is one of this module's types, which carry their carrier,
//! so `context_carrier` finds the carrier for exactly the same objects (across recorder instances, and after
//! settlement). The recorder's `metrics` and `propagation` are the recorder itself, as in TS; it is created
//! behind an `Rc` (`InMemoryTelemetryContext::new`) so it can hand out `Rc` handles to itself.

use std::any::Any;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string};
use pi_host::{LocalBoxFuture, PiError, PiResult, monotonic_ms, now_ms};
use serde::{Deserialize, Serialize};

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

fn copy_attributes(attributes: Option<&SpanAttributes>) -> SpanAttributes {
	attributes
		.into_iter()
		.flatten()
		.filter(|(_, value)| value.is_some())
		.map(|(name, value)| (name.clone(), value.clone()))
		.collect()
}

fn copy_link(link: &SpanLink) -> Option<RecordedTelemetryLink> {
	context_carrier(link.context.as_ref()).map(|context| RecordedTelemetryLink {
		context: context.clone(),
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
		parent_context: parent.map(|parent| parent.context.clone()),
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
		for (name, value) in copy_attributes(Some(attributes)) {
			recorded.attributes.insert(name, value);
		}
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

	let future = callback(span);
	Box::pin(async move {
		let result = future.await;
		let mut span = recorded.borrow_mut();
		if let Err(error) = &result
			&& !span.explicit_status
		{
			span.status = SpanStatus::Error {
				error: Some(crate::SpanStatusError {
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
		drop(state);
		drop(span);
		result
	})
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
			context: carrier.cloned(),
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
			Some(cursor) => Some(serde_json::from_str::<(f64, String)>(cursor)?),
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
					&& after.as_ref().is_none_or(|(started_ms, trace_id)| {
						first.started_ms < *started_ms
							|| (first.started_ms == *started_ms
								&& utf16_cmp(&trace.trace_id, trace_id) == Ordering::Greater)
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
		context_carrier(context).cloned()
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
