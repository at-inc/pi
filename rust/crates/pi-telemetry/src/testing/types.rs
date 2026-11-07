//! Port of `packages/telemetry/src/testing/types.ts`.
//! PORT-STATUS: reviewed

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use crate::memory::{RecordedTelemetryMeasurement, RecordedTelemetrySpan};
use crate::{TelemetryContext, TelemetryMetrics, TelemetryPropagation};

/// The `getMetrics()` method of a fixture.
pub type TelemetryAdapterMetricsReader =
	Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<Vec<RecordedTelemetryMeasurement>>>>;

/// A fresh adapter instance and normalized snapshot reader owned by one conformance case.
pub trait TelemetryAdapterFixture {
	fn context(&self) -> Rc<dyn TelemetryContext>;
	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		None
	}
	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		None
	}
	/// The optional `getMetrics?()` method, read without calling it: `None` when the fixture has no measurement
	/// reader.
	fn get_metrics(&self) -> Option<TelemetryAdapterMetricsReader> {
		None
	}
	fn get_spans(&self) -> LocalBoxFuture<'static, PiResult<Vec<RecordedTelemetrySpan>>>;
	/// `[Symbol.asyncDispose]()`.
	fn async_dispose(&self) -> LocalBoxFuture<'static, PiResult<()>>;
}

/// Creates an isolated adapter fixture for one conformance case.
pub type TelemetryAdapterFixtureFactory =
	Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<Rc<dyn TelemetryAdapterFixture>>>>;

/// The `run()` body of a conformance case.
pub type TelemetryAdapterConformanceRun = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<()>>>;

/// A runner-independent conformance case that can be registered with any test framework.
#[derive(Clone)]
pub struct TelemetryAdapterConformanceCase {
	pub group: String,
	pub name: String,
	run: TelemetryAdapterConformanceRun,
}

impl TelemetryAdapterConformanceCase {
	/// A case named `name` in `group` that runs `run`.
	pub fn new(group: impl Into<String>, name: impl Into<String>, run: TelemetryAdapterConformanceRun) -> Self {
		Self {
			group: group.into(),
			name: name.into(),
			run,
		}
	}

	/// `run()`.
	pub fn run(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		(self.run)()
	}
}
