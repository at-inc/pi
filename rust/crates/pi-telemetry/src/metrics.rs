//! Port of `packages/telemetry/src/metrics.ts`.
//! PORT-STATUS: reviewed
//!
//! The schema-driven type helpers (`MetricSchemas`, `MetricsOf`, `MetricNames`, `MetricByName`,
//! `DuplicateMetricNames`, `MetricAttributes`, `MeasurementArguments`) only narrow names and attributes at compile
//! time and are dropped; `SchemaTelemetryCounter` and `SchemaTelemetryHistogram` keep their names as aliases of
//! the untyped instruments.

use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{SpanAttributes, TelemetryContext, TelemetrySchemaDefinition, TelemetryStartAttributeDefinition};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstrumentOptions {
	pub description: Option<String>,
	pub unit: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistogramOptions {
	pub base: InstrumentOptions,
	/// Bucket hints; callers supply finite, strictly increasing values.
	pub boundaries: Option<Vec<f64>>,
}

impl Deref for HistogramOptions {
	type Target = InstrumentOptions;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for HistogramOptions {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

pub trait TelemetryCounter {
	fn add(&self, value: f64, attributes: Option<&SpanAttributes>);
}

pub trait TelemetryUpDownCounter {
	fn add(&self, value: f64, attributes: Option<&SpanAttributes>);
}

pub trait TelemetryHistogram {
	fn record(&self, value: f64, attributes: Option<&SpanAttributes>, context: Option<&dyn TelemetryContext>);
}

/// Optional adapter capability with synchronous factories and recording methods.
/// Typed inputs are trusted; callers supply finite measurements and nonnegative counter increments.
/// Malformed input behavior is unspecified.
pub trait TelemetryMetrics {
	fn counter(&self, name: &str, options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryCounter>;
	fn up_down_counter(&self, name: &str, options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryUpDownCounter>;
	fn histogram(&self, name: &str, options: Option<&HistogramOptions>) -> Rc<dyn TelemetryHistogram>;
}

/// `TelemetryStartAttributeDefinition & { cardinality?: "low" }`; the narrower cardinality is type-level only.
pub type TelemetryMetricAttributeDefinition = TelemetryStartAttributeDefinition;

/// `TelemetryMetricDefinition["kind"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetricKind {
	#[serde(rename = "counter")]
	Counter,
	#[serde(rename = "upDownCounter")]
	UpDownCounter,
	#[serde(rename = "histogram")]
	Histogram,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryMetricDefinition {
	pub description: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub unit: Option<String>,
	pub attributes: IndexMap<String, TelemetryMetricAttributeDefinition>,
	pub kind: MetricKind,
	/// Only histograms declare boundaries.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub boundaries: Option<Vec<f64>>,
}

/// `SchemaTelemetryCounter<Definition>`: a counter whose attributes the schema narrows at compile time.
pub type SchemaTelemetryCounter = dyn TelemetryCounter;

/// `SchemaTelemetryHistogram<Definition>`: a histogram whose attributes the schema narrows at compile time.
pub type SchemaTelemetryHistogram = dyn TelemetryHistogram;

/// Instruments bound to the metric definitions of one or more schemas.
#[derive(Clone)]
pub struct TypedTelemetryMetrics {
	metrics: Rc<dyn TelemetryMetrics>,
	definitions: IndexMap<String, TelemetryMetricDefinition>,
}

impl TypedTelemetryMetrics {
	pub fn counter(&self, name: &str) -> Rc<SchemaTelemetryCounter> {
		self.metrics.counter(name, self.instrument_options(name).as_ref())
	}

	pub fn up_down_counter(&self, name: &str) -> Rc<dyn TelemetryUpDownCounter> {
		self.metrics
			.up_down_counter(name, self.instrument_options(name).as_ref())
	}

	pub fn histogram(&self, name: &str) -> Rc<SchemaTelemetryHistogram> {
		let options = self.definitions.get(name).map(|definition| HistogramOptions {
			base: InstrumentOptions {
				description: Some(definition.description.clone()),
				unit: definition.unit.clone(),
			},
			boundaries: definition.boundaries.clone(),
		});
		self.metrics.histogram(name, options.as_ref())
	}

	/// The definition passed as `InstrumentOptions` (`metrics.counter(name, definitions.get(name))`).
	fn instrument_options(&self, name: &str) -> Option<InstrumentOptions> {
		self.definitions.get(name).map(|definition| InstrumentOptions {
			description: Some(definition.description.clone()),
			unit: definition.unit.clone(),
		})
	}
}

/// Bind instrument names, kinds, metadata, and exact measurement attributes to one or more schemas.
pub fn create_typed_metrics(
	metrics: Rc<dyn TelemetryMetrics>,
	schemas: &[TelemetrySchemaDefinition],
) -> TypedTelemetryMetrics {
	let mut definitions = IndexMap::new();
	for schema in schemas {
		for (name, definition) in schema.metrics.iter().flatten() {
			definitions.insert(name.clone(), definition.clone());
		}
	}
	TypedTelemetryMetrics { metrics, definitions }
}
