//! Port of `packages/telemetry/src/noop.ts`.
//! PORT-STATUS: reviewed
//!
//! The shared frozen objects become thread-local singletons (the core is single-threaded and `Rc` is `!Send`),
//! so every read on a thread returns the same `Rc`, like reading the TS constant. Read them with
//! `NOOP_TELEMETRY_CONTEXT.with(Rc::clone)`.

use std::any::Any;
use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use crate::metrics::{
	HistogramOptions, InstrumentOptions, TelemetryCounter, TelemetryHistogram, TelemetryMetrics, TelemetryUpDownCounter,
};
use crate::{
	SpanAttributes, SpanCallback, SpanLink, SpanOptions, SpanStatus, TelemetryCarrier, TelemetryContext,
	TelemetryPropagation, TelemetrySpan,
};

struct NoopCounter;

impl TelemetryCounter for NoopCounter {
	fn add(&self, _value: f64, _attributes: Option<&SpanAttributes>) {}
}

struct NoopUpDownCounter;

impl TelemetryUpDownCounter for NoopUpDownCounter {
	fn add(&self, _value: f64, _attributes: Option<&SpanAttributes>) {}
}

struct NoopHistogram;

impl TelemetryHistogram for NoopHistogram {
	fn record(&self, _value: f64, _attributes: Option<&SpanAttributes>, _context: Option<&dyn TelemetryContext>) {}
}

thread_local! {
	static NOOP_COUNTER: Rc<dyn TelemetryCounter> = Rc::new(NoopCounter);
	static NOOP_UP_DOWN_COUNTER: Rc<dyn TelemetryUpDownCounter> = Rc::new(NoopUpDownCounter);
	static NOOP_HISTOGRAM: Rc<dyn TelemetryHistogram> = Rc::new(NoopHistogram);
}

struct NoopTelemetryMetrics;

impl TelemetryMetrics for NoopTelemetryMetrics {
	fn counter(&self, _name: &str, _options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryCounter> {
		NOOP_COUNTER.with(Rc::clone)
	}

	fn up_down_counter(&self, _name: &str, _options: Option<&InstrumentOptions>) -> Rc<dyn TelemetryUpDownCounter> {
		NOOP_UP_DOWN_COUNTER.with(Rc::clone)
	}

	fn histogram(&self, _name: &str, _options: Option<&HistogramOptions>) -> Rc<dyn TelemetryHistogram> {
		NOOP_HISTOGRAM.with(Rc::clone)
	}
}

struct NoopTelemetryPropagation;

impl TelemetryPropagation for NoopTelemetryPropagation {
	fn inject(&self, _context: &dyn TelemetryContext) -> Option<TelemetryCarrier> {
		None
	}

	fn extract(&self, _carrier: Option<&TelemetryCarrier>) -> Rc<dyn TelemetryContext> {
		NOOP_TELEMETRY_CONTEXT.with(|context| context.clone())
	}
}

thread_local! {
	/// Shared instruments that never inspect or retain metadata, measurements, or contexts.
	pub static NOOP_TELEMETRY_METRICS: Rc<dyn TelemetryMetrics> = Rc::new(NoopTelemetryMetrics);
	/// Shared propagation capability that neither inspects nor retains its inputs.
	pub static NOOP_TELEMETRY_PROPAGATION: Rc<dyn TelemetryPropagation> = Rc::new(NoopTelemetryPropagation);
}

struct NoopTelemetrySpan;

impl TelemetryContext for NoopTelemetrySpan {
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		Some(NOOP_TELEMETRY_METRICS.with(Rc::clone))
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		Some(NOOP_TELEMETRY_PROPAGATION.with(Rc::clone))
	}

	fn start_span_erased(
		&self,
		_options: SpanOptions,
		callback: SpanCallback,
	) -> LocalBoxFuture<'static, PiResult<Box<dyn Any>>> {
		callback(NOOP_TELEMETRY_CONTEXT.with(Rc::clone))
	}
}

impl TelemetrySpan for NoopTelemetrySpan {
	fn add_link(&self, _link: &SpanLink) {}

	fn add_event(&self, _name: &str, _attributes: Option<&SpanAttributes>) {}

	fn set_attributes(&self, _attributes: &SpanAttributes) {}

	fn set_status(&self, _status: &SpanStatus) {}
}

thread_local! {
	/// Shared telemetry context used when an application does not provide one.
	pub static NOOP_TELEMETRY_CONTEXT: Rc<dyn TelemetrySpan> = Rc::new(NoopTelemetrySpan);
}
