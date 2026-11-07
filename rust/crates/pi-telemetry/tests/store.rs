//! Port of `packages/telemetry/test/store.test.ts`. `vi.spyOn(Date, "now")` and `vi.spyOn(performance, "now")`
//! move the scripted host's one virtual clock, which backs both `now_ms` and `monotonic_ms`.

mod common;
mod store_conformance;

use std::future::ready;
use std::rc::Rc;

use common::{options, set_now};
use pi_telemetry::{
	InMemoryTelemetryContext, MetricKind, MetricQuery, RecordedTelemetryEvent, RecordedTelemetryMeasurement, SpanLink,
	SpanOptions, SpanStatus, SpanStatusError, StoredSpan, StoredSpanEvent, StoredTrace, TelemetryContextExt,
	TelemetryPropagation, TelemetrySpan, TelemetryStore, TraceQuery,
};
use store_conformance::{StoreFixture, StoreFixtureFactory};

fn in_memory_factory() -> StoreFixtureFactory {
	Rc::new(|| {
		let context = InMemoryTelemetryContext::new();
		Box::pin(ready(StoreFixture {
			context: context.clone(),
			metrics: context.clone(),
			propagation: context.clone(),
			store: context,
			flush: Rc::new(|| Box::pin(ready(()))),
		}))
	})
}

telemetry_store_conformance!(in_memory_telemetry_store_conformance, in_memory_factory());

mod in_memory_store_snapshots {
	use super::*;

	#[test]
	fn records_wall_clock_boundaries_and_relative_event_times_without_changing_normalized_event_snapshots() {
		pi_host::testing::run(|host| async move {
			set_now(&host, 1000.0);
			let telemetry = InMemoryTelemetryContext::new();
			let id = Rc::new(std::cell::RefCell::new(String::new()));
			let slot = id.clone();
			let injector = telemetry.clone();
			let clock = host.clone();
			telemetry
				.start_span(options("timed"), move |span| {
					let carrier = injector.inject(span.as_ref()).unwrap();
					*slot.borrow_mut() = format!("{}:{}", carrier["pi-memory-scope"], carrier["pi-memory-span"]);
					clock.advance(10.0);
					span.add_event("event", Some(&attrs! { "count" => 1.0 }));
					clock.advance(15.0);
					ready(Ok(()))
				})
				.await
				.unwrap();
			let id = id.borrow().clone();
			assert_eq!(
				telemetry.get_trace(&id).await.unwrap(),
				Some(StoredTrace {
					trace_id: id.clone(),
					spans: vec![StoredSpan {
						id: id.clone(),
						trace_id: id.clone(),
						parent_id: None,
						name: "timed".to_string(),
						started_ms: 1000.0,
						ended_ms: 1025.0,
						attributes: attrs! {},
						events: vec![StoredSpanEvent {
							name: "event".to_string(),
							time_ms: 10.0,
							attributes: attrs! { "count" => 1.0 },
						}],
						links: vec![],
						status: SpanStatus::Ok,
					}],
				})
			);
			assert_eq!(
				telemetry.get_spans()[0].events,
				vec![RecordedTelemetryEvent {
					name: "event".to_string(),
					attributes: attrs! { "count" => 1.0 },
				}]
			);
			let snapshot = serde_json::to_value(&telemetry.get_spans()[0]).unwrap();
			assert!(snapshot.get("startedMs").is_none());
		});
	}

	#[test]
	fn maps_raw_metric_measurements_to_count_one_rows_without_exposing_private_timing_in_get_metrics() {
		pi_host::testing::run(|host| async move {
			set_now(&host, 20_001.0);
			let telemetry = InMemoryTelemetryContext::new();
			pi_telemetry::TelemetryMetrics::counter(telemetry.as_ref(), "calls", None).add(2.0, None);
			pi_telemetry::TelemetryMetrics::counter(telemetry.as_ref(), "calls", None).add(3.0, None);
			assert_eq!(
				telemetry
					.find_metrics(MetricQuery {
						name: "calls".to_string(),
						..MetricQuery::default()
					})
					.await
					.unwrap()
					.iter()
					.map(|row| (row.interval_ms, row.count, row.sum))
					.collect::<Vec<_>>(),
				vec![(20_000.0, 1.0, 2.0), (20_000.0, 1.0, 3.0)]
			);
			let measurement = |value: f64| RecordedTelemetryMeasurement {
				kind: MetricKind::Counter,
				name: "calls".to_string(),
				description: None,
				unit: None,
				boundaries: None,
				value,
				attributes: attrs! {},
				context: None,
			};
			assert_eq!(telemetry.get_metrics(), vec![measurement(2.0), measurement(3.0)]);
		});
	}

	#[test]
	fn detaches_link_attributes_and_error_details() {
		pi_host::testing::run(|_| async move {
			let telemetry = InMemoryTelemetryContext::new();
			let target: Rc<std::cell::RefCell<Option<Rc<dyn TelemetrySpan>>>> = Rc::default();
			let slot = target.clone();
			telemetry
				.start_span(options("target"), move |span| {
					*slot.borrow_mut() = Some(span);
					ready(Ok(()))
				})
				.await
				.unwrap();
			let target = target.borrow().clone().unwrap();
			telemetry
				.start_span(
					SpanOptions {
						name: "source".to_string(),
						attributes: None,
						links: Some(vec![SpanLink {
							context: target,
							attributes: Some(attrs! { "tags" => vec!["original"] }),
						}]),
					},
					|span| {
						span.set_status(&SpanStatus::Error {
							error: Some(SpanStatusError {
								name: "Expected".to_string(),
								message: "original".to_string(),
							}),
						});
						ready(Ok(()))
					},
				)
				.await
				.unwrap();
			let mut trace = telemetry
				.find_traces(Some(TraceQuery {
					name: Some("source".to_string()),
					..TraceQuery::default()
				}))
				.await
				.unwrap()
				.traces
				.remove(0);
			let span = &mut trace.spans[0];
			span.links[0]
				.attributes
				.as_mut()
				.unwrap()
				.insert("tags".to_string(), Some(vec!["changed"].into()));
			if let SpanStatus::Error { error: Some(error) } = &mut span.status {
				error.message = "changed".to_string();
			}
			let fresh = telemetry
				.get_trace(&trace.trace_id)
				.await
				.unwrap()
				.unwrap()
				.spans
				.remove(0);
			assert_eq!(
				fresh.links[0].attributes.as_ref().unwrap().get("tags"),
				Some(&Some(vec!["original"].into()))
			);
			assert_eq!(
				fresh.status,
				SpanStatus::Error {
					error: Some(SpanStatusError {
						name: "Expected".to_string(),
						message: "original".to_string(),
					}),
				}
			);
		});
	}
}
