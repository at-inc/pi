//! Port of `packages/telemetry/src/index.ts`.
//! PORT-STATUS: reviewed
//!
//! Type-level helpers with no runtime effect are dropped: `AttributeDefinitionValue`, `RequiredAttributeNames`,
//! `OptionalAttributeNames`, `InferRequiredAndOptionalAttributes`, `InferStartAttributes`,
//! `InferOptionalAttributes`, `ExactTelemetryAttributes`, `InferEventAttributes`, `TelemetrySchemaSpanName`,
//! `TelemetrySchemaSpanStartAttributes`, `TelemetrySchemaSpanEndAttributes`, `TelemetrySchemaSpanEventName`,
//! `TelemetrySchemaSpanEventAttributes`, `TelemetrySchemaSpanUnion`, `UniqueTelemetrySchemas`, and the private
//! span-name and union helpers. `SchemaTelemetrySpan` keeps its name as an alias of `dyn TelemetrySpan`.
//!
//! `TelemetryContext.startSpan<T>` is generic, which a trait object cannot be: adapters implement the
//! type-erased `start_span_erased`, and callers use the generic `TelemetryContextExt::start_span`. Adapters
//! invoke the callback inside `start_span_erased`, before returning, but a Rust callback returns a lazy future, so
//! that call only runs the closure prologue (the code before its `async` block). The in-memory adapter also polls
//! the returned future once before returning, which runs the body up to its first pending await, and settles the
//! span when the callback resolves even if the caller drops the returned future. The no-op context and inert spans
//! run the body only when the caller awaits it.
//!
//! A `PiError` whose `name` is the empty string stands for a thrown value that is not an `Error` (`throw {}`,
//! `Promise.reject(undefined)`); its `message` is `String(value)`. Recorders treat it as `!(error instanceof
//! Error)`.
//!
//! Schema definitions are typed structs whose fields follow the TS declaration order, so they serialize in that order
//! rather than the author's. Unknown keys and mixed-type attribute arrays, which the TS types do not allow, are not
//! supported: deserialization drops unknown keys and rejects mixed arrays.

use std::any::Any;
use std::future::Future;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::{LocalBoxFuture, PiResult};
use serde::{Deserialize, Serialize};

// <generated-modules>
pub mod memory;
pub mod metrics;
pub mod noop;
pub mod render;
pub mod testing;
// </generated-modules>

/// `string | number | boolean | readonly string[] | readonly number[] | readonly boolean[]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
	String(String),
	Number(f64),
	Boolean(bool),
	StringArray(Vec<String>),
	NumberArray(Vec<f64>),
	BooleanArray(Vec<bool>),
}

impl AttributeValue {
	/// JavaScript `===` against a query value: scalars compare by value, arrays never compare equal because
	/// `===` compares array identity and stored attributes are always copies.
	pub fn strict_equals(&self, other: &AttributeValue) -> bool {
		match (self, other) {
			(AttributeValue::String(left), AttributeValue::String(right)) => left == right,
			(AttributeValue::Number(left), AttributeValue::Number(right)) => left == right,
			(AttributeValue::Boolean(left), AttributeValue::Boolean(right)) => left == right,
			_ => false,
		}
	}
}

impl From<&str> for AttributeValue {
	fn from(value: &str) -> Self {
		AttributeValue::String(value.to_string())
	}
}

impl From<String> for AttributeValue {
	fn from(value: String) -> Self {
		AttributeValue::String(value)
	}
}

impl From<f64> for AttributeValue {
	fn from(value: f64) -> Self {
		AttributeValue::Number(value)
	}
}

impl From<bool> for AttributeValue {
	fn from(value: bool) -> Self {
		AttributeValue::Boolean(value)
	}
}

impl From<Vec<String>> for AttributeValue {
	fn from(value: Vec<String>) -> Self {
		AttributeValue::StringArray(value)
	}
}

impl From<Vec<&str>> for AttributeValue {
	fn from(value: Vec<&str>) -> Self {
		AttributeValue::StringArray(value.into_iter().map(str::to_string).collect())
	}
}

impl From<Vec<f64>> for AttributeValue {
	fn from(value: Vec<f64>) -> Self {
		AttributeValue::NumberArray(value)
	}
}

impl From<Vec<bool>> for AttributeValue {
	fn from(value: Vec<bool>) -> Self {
		AttributeValue::BooleanArray(value)
	}
}

/// `{ [name: string]: AttributeValue | undefined }`. `None` is an explicit `undefined` value; recorders drop it.
pub type SpanAttributes = IndexMap<String, Option<AttributeValue>>;

/// Opaque, JSON-serializable adapter-owned propagation data.
pub type TelemetryCarrier = IndexMap<String, String>;

/// Optional transport capability implemented by the application's telemetry adapter.
pub trait TelemetryPropagation {
	fn inject(&self, context: &dyn TelemetryContext) -> Option<TelemetryCarrier>;
	/// Absent or unknown carriers use the root; recognized identifiers are opaque strings.
	fn extract(&self, carrier: Option<&TelemetryCarrier>) -> Rc<dyn TelemetryContext>;
}

#[derive(Clone)]
pub struct SpanLink {
	pub context: Rc<dyn TelemetryContext>,
	pub attributes: Option<SpanAttributes>,
}

#[derive(Clone, Default)]
pub struct SpanOptions {
	pub name: String,
	pub attributes: Option<SpanAttributes>,
	pub links: Option<Vec<SpanLink>>,
}

/// The `error` payload of an error `SpanStatus`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanStatusError {
	pub name: String,
	pub message: String,
}

/// `{ status: "ok" } | { status: "error"; error?: { name: string; message: string } }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum SpanStatus {
	#[serde(rename = "ok")]
	Ok,
	#[serde(rename = "error")]
	Error {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		error: Option<SpanStatusError>,
	},
}

/// One entry of `StoredSpan.events`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSpanEvent {
	pub name: String,
	pub time_ms: f64,
	pub attributes: SpanAttributes,
}

/// One entry of `StoredSpan.links`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSpanLink {
	pub trace_id: String,
	pub span_id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub attributes: Option<SpanAttributes>,
}

/// A settled span snapshot, independent of the recording adapter. Event times are relative to span start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSpan {
	pub id: String,
	pub trace_id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub parent_id: Option<String>,
	pub name: String,
	pub started_ms: f64,
	pub ended_ms: f64,
	pub attributes: SpanAttributes,
	pub events: Vec<StoredSpanEvent>,
	pub links: Vec<StoredSpanLink>,
	pub status: SpanStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTrace {
	pub trace_id: String,
	pub spans: Vec<StoredSpan>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraceQuery {
	/// Name of the earliest stored span, with span ID breaking timestamp ties.
	pub name: Option<String>,
	/// All equalities must match one span; that span may be anywhere in the trace.
	pub attributes: Option<IndexMap<String, AttributeValue>>,
	/// `errors?: true`.
	pub errors: Option<bool>,
	/// Inclusive bounds on the earliest stored span's start time.
	pub start_min: Option<f64>,
	pub start_max: Option<f64>,
	/// Positive page size; defaults to 20.
	pub limit: Option<f64>,
	/// Opaque continuation from the previous page; retain the same filters.
	pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricQuery {
	pub name: String,
	/// Inclusive bounds on interval start time.
	pub start_min: Option<f64>,
	pub start_max: Option<f64>,
	pub attributes: Option<IndexMap<String, AttributeValue>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMetric {
	pub interval_ms: f64,
	pub name: String,
	pub kind: MetricKind,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub unit: Option<String>,
	pub attributes: SpanAttributes,
	pub count: f64,
	pub sum: f64,
	pub min: f64,
	pub max: f64,
}

/// The result of `TelemetryStore.findTraces`: `{ traces: readonly StoredTrace[]; cursor?: string }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FindTracesResult {
	pub traces: Vec<StoredTrace>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cursor: Option<String>,
}

/// Read settled traces without depending on a storage backend.
pub trait TelemetryStore {
	fn find_traces(&self, query: Option<TraceQuery>) -> LocalBoxFuture<'static, PiResult<FindTracesResult>>;
	fn get_trace(&self, trace_id: &str) -> LocalBoxFuture<'static, PiResult<Option<StoredTrace>>>;
	fn find_metrics(&self, query: MetricQuery) -> LocalBoxFuture<'static, PiResult<Vec<StoredMetric>>>;
}

/// The type-erased `startSpan` callback: `(span: TelemetrySpan) => T | Promise<T>` with `T` boxed as `Any`.
pub type SpanCallback = Box<dyn FnOnce(Rc<dyn TelemetrySpan>) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>>>;

/// Adapter contract for trusted typed inputs; malformed input behavior is unspecified.
pub trait TelemetryContext: Any {
	/// Adapter capabilities remain available on child spans and extracted parents.
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		None
	}
	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		None
	}
	/// `startSpan(options, callback)` with the result type erased. Implementations call `callback`
	/// synchronously, before returning, and resolve with its result or rejection.
	fn start_span_erased(
		&self,
		options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>>;
}

/// The generic `startSpan<T>` on every `TelemetryContext`, including `dyn TelemetryContext` and `dyn TelemetrySpan`.
pub trait TelemetryContextExt: TelemetryContext {
	/// `context.startSpan(options, callback)`.
	fn start_span<T, F, Fut>(&self, options: SpanOptions, callback: F) -> LocalBoxFuture<'static, PiResult<T>>
	where
		T: 'static,
		F: FnOnce(Rc<dyn TelemetrySpan>) -> Fut + 'static,
		Fut: Future<Output = PiResult<T>> + 'static,
	{
		let erased = self.start_span_erased(
			options,
			Box::new(move |span| {
				let future = callback(span);
				Box::pin(async move { future.await.map(|value| Box::new(value) as Box<dyn Any>) })
			}),
		);
		Box::pin(async move {
			erased.await.map(|value| {
				*value
					.downcast::<T>()
					.expect("startSpan resolves with the callback's own result")
			})
		})
	}
}

impl<C: TelemetryContext + ?Sized> TelemetryContextExt for C {}

pub trait TelemetrySpan: TelemetryContext {
	fn add_link(&self, link: &SpanLink);
	fn add_event(&self, name: &str, attributes: Option<&SpanAttributes>);
	fn set_attributes(&self, attributes: &SpanAttributes);
	fn set_status(&self, status: &SpanStatus);
}

pub use metrics::{
	HistogramOptions, InstrumentOptions, MetricKind, SchemaTelemetryCounter, SchemaTelemetryHistogram,
	TelemetryCounter, TelemetryHistogram, TelemetryMetricAttributeDefinition, TelemetryMetricDefinition,
	TelemetryMetrics, TelemetryUpDownCounter, TypedTelemetryMetrics, create_typed_metrics,
};
pub use noop::{NOOP_TELEMETRY_CONTEXT, NOOP_TELEMETRY_METRICS, NOOP_TELEMETRY_PROPAGATION};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TelemetryAttributeType {
	#[serde(rename = "string")]
	String,
	#[serde(rename = "number")]
	Number,
	#[serde(rename = "boolean")]
	Boolean,
	#[serde(rename = "string[]")]
	StringArray,
	#[serde(rename = "number[]")]
	NumberArray,
	#[serde(rename = "boolean[]")]
	BooleanArray,
}

/// `cardinality?: "low" | "high"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TelemetryAttributeCardinality {
	#[serde(rename = "low")]
	Low,
	#[serde(rename = "high")]
	High,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryAttributeMetadata {
	pub description: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sensitive: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cardinality: Option<TelemetryAttributeCardinality>,
}

/// The `type`-tagged half of `TelemetryAttributeDefinition`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TelemetryAttributeValueDefinition {
	#[serde(rename = "string")]
	String {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		values: Option<Vec<String>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		examples: Option<Vec<String>>,
	},
	#[serde(rename = "number")]
	Number {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		values: Option<Vec<f64>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		examples: Option<Vec<f64>>,
	},
	#[serde(rename = "boolean")]
	Boolean {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		values: Option<Vec<bool>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		examples: Option<Vec<bool>>,
	},
	#[serde(rename = "string[]")]
	StringArray {
		#[serde(default, rename = "elementValues", skip_serializing_if = "Option::is_none")]
		element_values: Option<Vec<String>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		examples: Option<Vec<Vec<String>>>,
	},
	#[serde(rename = "number[]")]
	NumberArray {
		#[serde(default, rename = "elementValues", skip_serializing_if = "Option::is_none")]
		element_values: Option<Vec<f64>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		examples: Option<Vec<Vec<f64>>>,
	},
	#[serde(rename = "boolean[]")]
	BooleanArray {
		#[serde(default, rename = "elementValues", skip_serializing_if = "Option::is_none")]
		element_values: Option<Vec<bool>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		examples: Option<Vec<Vec<bool>>>,
	},
}

/// `TelemetryAttributeMetadata & ({ type: "string"; ... } | ...)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryAttributeDefinition {
	#[serde(flatten)]
	pub metadata: TelemetryAttributeMetadata,
	#[serde(flatten)]
	pub value: TelemetryAttributeValueDefinition,
}

/// `TelemetryAttributeDefinition & { required: boolean }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryStartAttributeDefinition {
	#[serde(flatten)]
	pub base: TelemetryAttributeDefinition,
	pub required: bool,
}

impl std::ops::Deref for TelemetryStartAttributeDefinition {
	type Target = TelemetryAttributeDefinition;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

/// `TelemetryAttributeDefinition & { required: boolean }`.
pub type TelemetryEventAttributeDefinition = TelemetryStartAttributeDefinition;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryEventDefinition {
	pub description: String,
	pub attributes: IndexMap<String, TelemetryEventAttributeDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TelemetryParentDefinition {
	#[serde(rename = "any")]
	Any,
	#[serde(rename = "root_or_external")]
	RootOrExternal,
	#[serde(rename = "spans")]
	Spans { spans: Vec<String> },
}

/// `default: "ok"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TelemetryDefaultSpanStatus {
	#[serde(rename = "ok")]
	Ok,
}

/// `TelemetrySpanDefinition.status`: `{ default: "ok"; errorWhen: string }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySpanStatusDefinition {
	pub default: TelemetryDefaultSpanStatus,
	pub error_when: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySpanDefinition {
	pub description: String,
	pub parents: TelemetryParentDefinition,
	pub start_attributes: IndexMap<String, TelemetryStartAttributeDefinition>,
	pub end_attributes: IndexMap<String, TelemetryAttributeDefinition>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub events: Option<IndexMap<String, TelemetryEventDefinition>>,
	pub status: TelemetrySpanStatusDefinition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetrySchemaDefinition {
	pub version: f64,
	pub spans: IndexMap<String, TelemetrySpanDefinition>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub metrics: Option<IndexMap<String, TelemetryMetricDefinition>>,
}

/// Typed identity helper for serializable telemetry schema data.
pub fn define_telemetry_schema(schema: TelemetrySchemaDefinition) -> TelemetrySchemaDefinition {
	schema
}

/// `SchemaTelemetrySpan<Schema, Name>`: the schema only narrows argument types, so at runtime it is a
/// `TelemetrySpan`.
pub type SchemaTelemetrySpan = dyn TelemetrySpan;

/// A per-span starter bound to one explicit parent context and one or more schemas.
#[derive(Clone)]
pub struct TypedSpanStarter {
	telemetry_context: Rc<dyn TelemetryContext>,
}

impl TypedSpanStarter {
	/// `startSpan(name, attributes, callback)`: the callback receives the span and a starter bound to it.
	pub fn start_span<T, F, Fut>(
		&self,
		name: &str,
		attributes: SpanAttributes,
		callback: F,
	) -> LocalBoxFuture<'static, PiResult<T>>
	where
		T: 'static,
		F: FnOnce(Rc<SchemaTelemetrySpan>, TypedSpanStarter) -> Fut + 'static,
		Fut: Future<Output = PiResult<T>> + 'static,
	{
		self.telemetry_context.start_span(
			SpanOptions {
				name: name.to_string(),
				attributes: Some(attributes),
				links: None,
			},
			move |span| {
				let child = bind_typed_span_starter(span.clone());
				callback(span, child)
			},
		)
	}
}

fn bind_typed_span_starter(telemetry_context: Rc<dyn TelemetryContext>) -> TypedSpanStarter {
	TypedSpanStarter { telemetry_context }
}

/// Bind an explicit parent context to the combined span vocabulary of one or more schemas.
/// Schema values are used only for type inference; no runtime schema validation is performed.
pub fn create_typed_span_starter(
	telemetry_context: Rc<dyn TelemetryContext>,
	_schemas: &[TelemetrySchemaDefinition],
) -> TypedSpanStarter {
	bind_typed_span_starter(telemetry_context)
}

pub use memory::{
	InMemoryTelemetryContext, RecordedTelemetryEvent, RecordedTelemetryLink, RecordedTelemetryMeasurement,
	RecordedTelemetrySpan,
};
