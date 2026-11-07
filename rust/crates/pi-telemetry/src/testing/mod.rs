//! Port of `packages/telemetry/src/testing/index.ts`.
//! PORT-STATUS: reviewed

// <generated-modules>
pub mod conformance;
pub mod types;
// </generated-modules>

pub use conformance::create_telemetry_adapter_conformance;
pub use types::{
	TelemetryAdapterConformanceCase, TelemetryAdapterFixture, TelemetryAdapterFixtureFactory,
	TelemetryAdapterMetricsReader,
};
