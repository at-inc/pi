//! Port of `packages/telemetry/test/metrics.test.ts`. The `@ts-expect-error` compile-time checks have no
//! runtime behavior and are not ported.

mod common;

use std::future::ready;
use std::rc::Rc;

use common::same_object;
use pi_telemetry::{
	HistogramOptions, InMemoryTelemetryContext, InstrumentOptions, MetricKind, NOOP_TELEMETRY_CONTEXT,
	NOOP_TELEMETRY_METRICS, TelemetryContextExt, TelemetrySchemaDefinition, create_typed_metrics,
	define_telemetry_schema,
};
use serde_json::json;

fn traffic() -> TelemetrySchemaDefinition {
	define_telemetry_schema(
		serde_json::from_value(json!({
			"version": 1,
			"spans": {},
			"metrics": {
				"network.bytes": {
					"kind": "counter",
					"description": "Encoded bytes",
					"unit": "By",
					"attributes": {
						"direction": {
							"type": "string",
							"values": ["in", "out"],
							"required": true,
							"description": "Direction",
							"cardinality": "low",
						},
					},
				},
				"network.active": { "kind": "upDownCounter", "description": "Open links", "attributes": {} },
			},
		}))
		.unwrap(),
	)
}

fn latency() -> TelemetrySchemaDefinition {
	define_telemetry_schema(
		serde_json::from_value(json!({
			"version": 1,
			"spans": {},
			"metrics": {
				"call.duration": {
					"kind": "histogram",
					"description": "Call duration",
					"unit": "ms",
					"boundaries": [1, 10, 100],
					"attributes": { "method": { "type": "string", "required": false, "description": "Method" } },
				},
			},
		}))
		.unwrap(),
	)
}

mod metrics {
	use super::*;

	#[test]
	fn binds_schema_names_kinds_metadata_and_exact_measurement_attributes() {
		pi_host::testing::run(|_| async {
			let adapter = InMemoryTelemetryContext::new();
			let traffic = traffic();
			let metrics = Rc::new(create_typed_metrics(adapter.clone(), &[traffic.clone(), latency()]));
			metrics
				.counter("network.bytes")
				.add(128.0, Some(&attrs! { "direction" => "out" }));
			metrics.up_down_counter("network.active").add(1.0, None);
			metrics.up_down_counter("network.active").add(-1.0, None);
			let recording = metrics.clone();
			adapter
				.start_span(common::options("request"), move |span| {
					recording.histogram("call.duration").record(
						12.0,
						Some(&attrs! { "method" => "read" }),
						Some(span.as_ref()),
					);
					recording.histogram("call.duration").record(13.0, None, None);
					recording
						.histogram("call.duration")
						.record(14.0, None, Some(span.as_ref()));
					ready(Ok(()))
				})
				.await
				.unwrap();
			let measurements = adapter.get_metrics();
			assert_eq!(measurements[0].kind, MetricKind::Counter);
			assert_eq!(measurements[0].name, "network.bytes");
			assert_eq!(measurements[0].description.as_deref(), Some("Encoded bytes"));
			assert_eq!(measurements[0].unit.as_deref(), Some("By"));
			assert_eq!(measurements[0].value, 128.0);
			assert_eq!(measurements[0].attributes, attrs! { "direction" => "out" });
			assert_eq!(measurements[3].kind, MetricKind::Histogram);
			assert_eq!(measurements[3].boundaries, Some(vec![1.0, 10.0, 100.0]));
			assert_eq!(measurements[3].context, Some(adapter.get_spans()[0].context.clone()));
			assert_eq!(measurements[4].context, None);
			assert_eq!(measurements[5].context, Some(adapter.get_spans()[0].context.clone()));
			pi_host::js::json_stringify(&traffic);
		});
	}

	#[test]
	fn requires_histogram_attributes_when_the_schema_declares_them() {
		pi_host::testing::run(|_| async {
			let schema = define_telemetry_schema(
				serde_json::from_value(json!({
					"version": 1,
					"spans": {},
					"metrics": {
						"required": {
							"kind": "histogram",
							"description": "Required tag",
							"attributes": {
								"cache": { "type": "boolean", "required": true, "description": "Cache hit" },
							},
						},
					},
				}))
				.unwrap(),
			);
			let metrics = create_typed_metrics(InMemoryTelemetryContext::new(), &[schema]);
			metrics
				.histogram("required")
				.record(1.0, Some(&attrs! { "cache" => true }), None);
		});
	}

	#[test]
	fn returns_detached_measurement_snapshots_including_exemplars_and_boundaries() {
		pi_host::testing::run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let histogram = pi_telemetry::TelemetryMetrics::histogram(
				telemetry.as_ref(),
				"duration",
				Some(&HistogramOptions {
					base: InstrumentOptions::default(),
					boundaries: Some(vec![1.0, 2.0]),
				}),
			);
			telemetry
				.start_span(common::options("request"), move |span| {
					histogram.record(1.0, Some(&attrs! { "tags" => vec!["original"] }), Some(span.as_ref()));
					ready(Ok(()))
				})
				.await
				.unwrap();
			let mut measurement = telemetry.get_metrics().remove(0);
			let Some(context) = measurement.context.as_mut() else {
				panic!("Expected measurement context");
			};
			context.insert("pi-memory-span".to_string(), "999".to_string());
			if let Some(boundaries) = measurement.boundaries.as_mut() {
				boundaries.push(3.0);
			}
			measurement
				.attributes
				.insert("tags".to_string(), Some(vec!["mutated"].into()));
			let fresh = telemetry.get_metrics().remove(0);
			assert_eq!(fresh.boundaries, Some(vec![1.0, 2.0]));
			assert_eq!(fresh.attributes, attrs! { "tags" => vec!["original"] });
			assert_eq!(fresh.context, Some(telemetry.get_spans()[0].context.clone()));
		});
	}

	#[test]
	fn shares_frozen_no_op_instruments() {
		pi_host::testing::run(|_| async {
			let metrics = NOOP_TELEMETRY_METRICS.with(Rc::clone);
			let options = InstrumentOptions {
				description: None,
				unit: Some("By".to_string()),
			};
			let attributes = attrs! { "direction" => "out" };
			let counter = metrics.counter("bytes", Some(&options));
			let active = metrics.up_down_counter("active", Some(&options));
			let histogram = metrics.histogram(
				"duration",
				Some(&HistogramOptions {
					base: options.clone(),
					boundaries: None,
				}),
			);
			assert!(same_object(&counter, &metrics.counter("other", None)));
			counter.add(1.0, Some(&attributes));
			active.add(-1.0, Some(&attributes));
			histogram.record(
				1.0,
				Some(&attributes),
				Some(NOOP_TELEMETRY_CONTEXT.with(Rc::clone).as_ref()),
			);
		});
	}
}
