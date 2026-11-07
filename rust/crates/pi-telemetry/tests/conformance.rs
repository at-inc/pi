//! Port of `packages/telemetry/test/conformance.test.ts`.

mod common;

use std::future::ready;
use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiError, PiResult};
use pi_telemetry::testing::{
	TelemetryAdapterFixture, TelemetryAdapterMetricsReader, create_telemetry_adapter_conformance,
};
use pi_telemetry::{
	AttributeValue, InMemoryTelemetryContext, RecordedTelemetryEvent, RecordedTelemetryMeasurement,
	RecordedTelemetrySpan, SpanOptions, TelemetryContext, TelemetryContextExt, TelemetryMetrics, TelemetryPropagation,
};
use serde_json::json;

struct InMemoryFixture {
	context: Rc<InMemoryTelemetryContext>,
}

impl TelemetryAdapterFixture for InMemoryFixture {
	fn context(&self) -> Rc<dyn TelemetryContext> {
		self.context.clone()
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		Some(self.context.clone())
	}

	fn metrics(&self) -> Option<Rc<dyn TelemetryMetrics>> {
		Some(self.context.clone())
	}

	fn get_metrics(&self) -> Option<TelemetryAdapterMetricsReader> {
		let context = self.context.clone();
		Some(Rc::new(move || {
			let metrics: Vec<RecordedTelemetryMeasurement> = context.get_metrics();
			Box::pin(ready(Ok(metrics)))
		}))
	}

	fn get_spans(&self) -> LocalBoxFuture<'static, PiResult<Vec<RecordedTelemetrySpan>>> {
		let spans = self.context.get_spans();
		Box::pin(ready(Ok(spans)))
	}

	fn async_dispose(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		Box::pin(ready(Ok(())))
	}
}

/// A fixture whose span reader and disposal can fail, for the `await using` completion rules.
struct FailingFixture {
	context: Rc<InMemoryTelemetryContext>,
	spans_error: Option<PiError>,
	dispose_error: Option<PiError>,
}

impl TelemetryAdapterFixture for FailingFixture {
	fn context(&self) -> Rc<dyn TelemetryContext> {
		self.context.clone()
	}

	fn propagation(&self) -> Option<Rc<dyn TelemetryPropagation>> {
		Some(self.context.clone())
	}

	fn get_spans(&self) -> LocalBoxFuture<'static, PiResult<Vec<RecordedTelemetrySpan>>> {
		let spans = match &self.spans_error {
			Some(error) => Err(error.clone()),
			None => Ok(self.context.get_spans()),
		};
		Box::pin(ready(spans))
	}

	fn async_dispose(&self) -> LocalBoxFuture<'static, PiResult<()>> {
		let disposed = match &self.dispose_error {
			Some(error) => Err(error.clone()),
			None => Ok(()),
		};
		Box::pin(ready(disposed))
	}
}

fn suppressed_error(error: serde_json::Value, suppressed: serde_json::Value) -> PiError {
	PiError::named("SuppressedError", "An error was suppressed during disposal.")
		.with_data(json!({ "error": error, "suppressed": suppressed }))
}

fn run_case(group: &str, name: &str) {
	pi_host::testing::run(|_| async {
		let conformance = create_telemetry_adapter_conformance(Rc::new(|| {
			let fixture: Rc<dyn TelemetryAdapterFixture> = Rc::new(InMemoryFixture {
				context: InMemoryTelemetryContext::new(),
			});
			Box::pin(ready(Ok(fixture)))
		}));
		let case = conformance
			.iter()
			.find(|case| case.group == group && case.name == name)
			.expect("conformance case exists");
		case.run().await.expect("conformance case passes");
	});
}

mod in_memory_telemetry_context_conformance {
	use super::*;

	mod capabilities {
		use super::super::run_case;

		#[test]
		fn carries_adapter_capabilities_through_contexts_and_spans() {
			run_case(
				"capabilities",
				"carries adapter capabilities through contexts and spans",
			);
		}
	}

	mod metrics {
		use super::super::run_case;

		#[test]
		fn records_ordered_measurements_and_optional_histogram_exemplars() {
			run_case(
				"metrics",
				"records ordered measurements and optional histogram exemplars",
			);
		}

		#[test]
		fn copies_instrument_metadata_and_measurement_attributes() {
			run_case("metrics", "copies instrument metadata and measurement attributes");
		}
	}

	mod links {
		use super::super::run_case;

		#[test]
		fn records_initial_and_late_links_without_changing_parents() {
			run_case("links", "records initial and late links without changing parents");
		}

		#[test]
		fn ignores_links_added_after_source_settlement() {
			run_case("links", "ignores links added after source settlement");
		}
	}

	mod propagation {
		use super::super::run_case;

		#[test]
		fn roundtrips_parent_identity_through_json_without_retaining_the_carrier() {
			run_case(
				"propagation",
				"roundtrips parent identity through JSON without retaining the carrier",
			);
		}

		#[test]
		fn absent_and_unknown_carriers_produce_recorded_roots() {
			run_case("propagation", "absent and unknown carriers produce recorded roots");
		}

		#[test]
		fn propagates_across_fresh_adapters_and_multiple_hops() {
			run_case("propagation", "propagates across fresh adapters and multiple hops");
		}
	}

	mod callback_lifecycle {
		use super::super::run_case;

		#[test]
		fn admits_once_synchronously_and_preserves_the_result() {
			run_case(
				"callback lifecycle",
				"admits once synchronously and preserves the result",
			);
		}

		#[test]
		fn preserves_synchronous_and_asynchronous_rejection_values() {
			run_case(
				"callback lifecycle",
				"preserves synchronous and asynchronous rejection values",
			);
		}
	}

	mod status {
		use super::super::run_case;

		#[test]
		fn uses_last_explicit_status_without_automatic_overwrite() {
			run_case("status", "uses last explicit status without automatic overwrite");
		}
	}

	mod recording {
		use super::super::run_case;

		#[test]
		fn merges_attributes_and_records_ordered_events() {
			run_case("recording", "merges attributes and records ordered events");
		}

		#[test]
		fn makes_calls_after_settlement_inert() {
			run_case("recording", "makes calls after settlement inert");
		}
	}

	mod parentage {
		use super::super::run_case;

		#[test]
		fn records_nested_and_concurrent_child_relationships() {
			run_case("parentage", "records nested and concurrent child relationships");
		}
	}

	#[test]
	fn reports_case_and_disposal_failures_as_a_suppressed_error() {
		pi_host::testing::run(|_| async {
			let conformance = create_telemetry_adapter_conformance(Rc::new(|| {
				let fixture: Rc<dyn TelemetryAdapterFixture> = Rc::new(FailingFixture {
					context: InMemoryTelemetryContext::new(),
					spans_error: Some(PiError::new("spans")),
					dispose_error: Some(PiError::new("dispose")),
				});
				Box::pin(ready(Ok(fixture)))
			}));
			let case = conformance
				.iter()
				.find(|case| case.name == "ignores links added after source settlement")
				.expect("conformance case exists");
			assert_eq!(
				case.run().await,
				Err(suppressed_error(
					json!({ "name": "Error", "message": "dispose" }),
					json!({ "name": "Error", "message": "spans" }),
				))
			);
		});
	}

	#[test]
	fn disposes_the_receiver_when_the_worker_factory_fails() {
		pi_host::testing::run(|_| async {
			let calls = Rc::new(std::cell::Cell::new(0));
			let conformance = create_telemetry_adapter_conformance(Rc::new(move || {
				calls.set(calls.get() + 1);
				if calls.get() == 3 {
					return Box::pin(ready(Err(PiError::new("worker factory"))));
				}
				let fixture: Rc<dyn TelemetryAdapterFixture> = Rc::new(FailingFixture {
					context: InMemoryTelemetryContext::new(),
					spans_error: None,
					dispose_error: (calls.get() == 2).then(|| PiError::new("receiver dispose")),
				});
				Box::pin(ready(Ok(fixture)))
			}));
			let case = conformance
				.iter()
				.find(|case| case.name == "propagates across fresh adapters and multiple hops")
				.expect("conformance case exists");
			assert_eq!(
				case.run().await,
				Err(suppressed_error(
					json!({ "name": "Error", "message": "receiver dispose" }),
					json!({ "name": "Error", "message": "worker factory" }),
				))
			);
		});
	}

	#[test]
	fn returns_detached_snapshots_without_exposing_mutable_recording_state() {
		pi_host::testing::run(|_| async {
			let context = InMemoryTelemetryContext::new();
			let open = std::rc::Rc::new(std::cell::RefCell::new((None, None)));
			let slot = open.clone();
			let reader = context.clone();
			context
				.start_span(
					SpanOptions {
						name: "snapshot".to_string(),
						attributes: Some(attrs! { "tags" => vec!["initial"] }),
						links: None,
					},
					move |span| {
						span.add_event("event", Some(&attrs! { "value" => 1.0 }));
						let spans = reader.get_spans();
						let first = spans.first();
						*slot.borrow_mut() = (first.map(|span| span.settled), first.and_then(|span| span.end_sequence));
						ready(Ok(()))
					},
				)
				.await
				.unwrap();

			let (open_settled, open_end_sequence) = *open.borrow();
			assert_eq!(open_settled, Some(false));
			assert_eq!(open_end_sequence, None);
			let mut first = context.get_spans().remove(0);
			assert!(first.settled);
			assert_eq!(first.end_sequence, Some(1.0));
			first
				.attributes
				.insert("tags".to_string(), Some(AttributeValue::from(vec!["mutated"])));
			first.events[0]
				.attributes
				.insert("value".to_string(), Some(AttributeValue::Number(2.0)));

			let second = context.get_spans().remove(0);
			assert_eq!(second.attributes, attrs! { "tags" => vec!["initial"] });
			assert_eq!(
				second.events,
				vec![RecordedTelemetryEvent {
					name: "event".to_string(),
					attributes: attrs! { "value" => 1.0 },
				}]
			);
		});
	}
}
