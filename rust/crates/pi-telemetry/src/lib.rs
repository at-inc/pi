//! Port of `packages/telemetry/src/index.ts`.
//! PORT-STATUS: pending

// <generated-modules>
pub mod memory;
pub mod metrics;
pub mod noop;
pub mod render;
pub mod testing;
// </generated-modules>

// PORT-STUB: w1-telemetry-mcp
/// `TelemetrySpan`, predicted as a trait that adapters implement and pi-ai holds as `Rc<dyn TelemetrySpan>`
/// (`ProviderRequestOptions.telemetry`). The owning shard replaces this with the real interface.
pub trait TelemetrySpan {}
