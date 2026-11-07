//! Port of `packages/telemetry/test/telemetry.test.ts`. The `expectTypeOf` and `@ts-expect-error` checks have
//! no runtime behavior and are not ported; `Object.isFrozen` has no Rust counterpart (the no-op span has no
//! mutable state).

mod common;

use std::cell::{Cell, RefCell};
use std::future::ready;
use std::rc::Rc;

use common::same_object;
use pi_host::PiError;
use pi_telemetry::{
	InMemoryTelemetryContext, NOOP_TELEMETRY_CONTEXT, NOOP_TELEMETRY_METRICS, NOOP_TELEMETRY_PROPAGATION, SpanLink,
	SpanStatus, SpanStatusError, TelemetryContext, TelemetryContextExt, TelemetryMetricDefinition,
	TelemetrySchemaDefinition, TelemetrySpan, TelemetryStartAttributeDefinition, create_typed_span_starter,
	define_telemetry_schema,
};
use serde_json::json;

mod telemetry_schemas {
	use super::*;

	#[test]
	fn preserves_serializable_definitions_and_infers_exact_attributes() {
		let definition: TelemetrySchemaDefinition = serde_json::from_value(json!({
			"version": 1,
			"spans": {
				"operation": {
					"description": "Test operation",
					"parents": { "kind": "any" },
					"startAttributes": {
						"kind": { "type": "string", "required": true, "values": ["read", "write"], "description": "Kind" },
					},
					"endAttributes": {},
					"events": {
						"result": {
							"description": "Result",
							"attributes": {
								"outcome": { "type": "string", "required": true, "values": ["ok", "error"], "description": "Outcome" },
							},
						},
					},
					"status": { "default": "ok", "errorWhen": "The operation fails" },
				},
			},
		}))
		.unwrap();
		let schema = define_telemetry_schema(definition.clone());
		assert_eq!(schema, definition);
		pi_host::js::json_stringify(&schema);
	}

	#[test]
	fn serializes_definitions_in_declaration_order() {
		let attribute: TelemetryStartAttributeDefinition = serde_json::from_value(json!({
			"type": "string", "description": "D", "values": ["a"], "required": true,
		}))
		.unwrap();
		assert_eq!(
			pi_host::js::json_stringify(&attribute),
			r#"{"description":"D","type":"string","values":["a"],"required":true}"#
		);
		let metric: TelemetryMetricDefinition = serde_json::from_value(json!({
			"kind": "histogram", "description": "Duration", "unit": "ms", "attributes": {}, "boundaries": [1, 10],
		}))
		.unwrap();
		assert_eq!(
			pi_host::js::json_stringify(&metric),
			r#"{"description":"Duration","unit":"ms","attributes":{},"kind":"histogram","boundaries":[1,10]}"#
		);
	}

	#[test]
	fn combines_schema_vocabularies_and_binds_child_starters_to_their_parent_spans() {
		pi_host::testing::run(|_| async {
			let operation_schema = define_telemetry_schema(
				serde_json::from_value(json!({
					"version": 1,
					"spans": {
						"operation": {
							"description": "Operation",
							"parents": { "kind": "root_or_external" },
							"startAttributes": {
								"kind": { "type": "string", "required": true, "values": ["read", "write"], "description": "Kind" },
							},
							"endAttributes": {},
							"status": { "default": "ok", "errorWhen": "The operation fails" },
						},
					},
				}))
				.unwrap(),
			);
			let request_schema = define_telemetry_schema(
				serde_json::from_value(json!({
					"version": 3,
					"spans": {
						"request": {
							"description": "Request",
							"parents": { "kind": "spans", "spans": ["operation"] },
							"startAttributes": {
								"provider": { "type": "string", "required": true, "description": "Provider" },
							},
							"endAttributes": {
								"response": { "type": "string", "description": "Response kind" },
							},
							"status": { "default": "ok", "errorWhen": "The request fails" },
						},
					},
				}))
				.unwrap(),
			);
			let telemetry_context = InMemoryTelemetryContext::new();
			let start_span = create_typed_span_starter(telemetry_context.clone(), &[operation_schema, request_schema]);

			let result = start_span
				.start_span(
					"operation",
					attrs! { "kind" => "read" },
					|_operation_span, start_child_span| {
						start_child_span.start_span("request", attrs! { "provider" => "example" }, |request_span, _| {
							request_span.set_attributes(&attrs! { "response" => "cached" });
							ready(Ok(42.0))
						})
					},
				)
				.await
				.unwrap();

			assert_eq!(result, 42.0);
			let spans = telemetry_context.get_spans();
			let operation_span = spans.iter().find(|span| span.name == "operation");
			let request_span = spans.iter().find(|span| span.name == "request");
			assert_eq!(operation_span.and_then(|span| span.parent_id), None);
			assert_eq!(
				request_span.and_then(|span| span.parent_id),
				operation_span.map(|span| span.id)
			);

			let sync_error = PiError::new("sync");
			let thrown = sync_error.clone();
			let sync_result = start_span.start_span("operation", attrs! { "kind" => "write" }, move |_, _| {
				ready(Err::<(), _>(thrown))
			});
			assert_eq!(sync_result.await, Err(sync_error));

			let async_error = PiError::new("async");
			let thrown = async_error.clone();
			let async_result =
				start_span.start_span("request", attrs! { "provider" => "example" }, move |_, _| async move {
					Err::<(), _>(thrown)
				});
			assert_eq!(async_result.await, Err(async_error));
		});
	}
}

mod noop_telemetry_context {
	use super::*;

	#[test]
	fn admits_callbacks_synchronously_and_reuses_one_inert_span() {
		pi_host::testing::run(|_| async {
			let admitted = Rc::new(Cell::new(false));
			let first_span: Rc<RefCell<Option<Rc<dyn TelemetrySpan>>>> = Rc::default();
			let context = NOOP_TELEMETRY_CONTEXT.with(Rc::clone);
			let result = {
				let admitted = admitted.clone();
				let first_span = first_span.clone();
				context.start_span(common::options("first"), move |span| {
					admitted.set(true);
					*first_span.borrow_mut() = Some(span.clone());
					async move {
						let child = span
							.start_span(common::options("child"), |child_span| ready(Ok(child_span)))
							.await?;
						assert!(same_object(&child, &span));
						Ok(42.0)
					}
				})
			};

			assert!(admitted.get());
			assert_eq!(result.await, Ok(42.0));
			let metrics = NOOP_TELEMETRY_METRICS.with(Rc::clone);
			let propagation = NOOP_TELEMETRY_PROPAGATION.with(Rc::clone);
			assert!(same_object(&context.metrics().unwrap(), &metrics));
			assert!(same_object(&context.propagation().unwrap(), &propagation));
			let first_span = first_span.borrow().clone().unwrap();
			assert!(same_object(&first_span.metrics().unwrap(), &metrics));
			assert!(same_object(&first_span.propagation().unwrap(), &propagation));
		});
	}

	#[test]
	fn preserves_synchronous_and_asynchronous_rejection_values() {
		pi_host::testing::run(|_| async {
			let context = NOOP_TELEMETRY_CONTEXT.with(Rc::clone);
			let sync_error = PiError::new("sync");
			let thrown = sync_error.clone();
			let sync = context.start_span(common::options("sync"), move |_| ready(Err::<(), _>(thrown)));
			assert_eq!(sync.await, Err(sync_error));

			let async_error = PiError::new("async");
			let thrown = async_error.clone();
			let async_result =
				context.start_span(common::options("async"), move |_| async move { Err::<(), _>(thrown) });
			assert_eq!(async_result.await, Err(async_error));
		});
	}

	#[test]
	fn ignores_typed_recording_payloads() {
		pi_host::testing::run(|_| async {
			let context = NOOP_TELEMETRY_CONTEXT.with(Rc::clone);
			let result = context
				.start_span(
					pi_telemetry::SpanOptions {
						name: "operation".to_string(),
						attributes: Some(attrs! { "kind" => "test" }),
						links: None,
					},
					|span| {
						span.add_event("event", Some(&attrs! { "count" => 1.0 }));
						span.set_attributes(&attrs! { "complete" => true });
						span.set_status(&SpanStatus::Error {
							error: Some(SpanStatusError {
								name: "Expected".to_string(),
								message: "test".to_string(),
							}),
						});
						let link_context: Rc<dyn TelemetryContext> = span.clone();
						span.add_link(&SpanLink {
							context: link_context,
							attributes: None,
						});
						ready(Ok(42.0))
					},
				)
				.await;
			assert_eq!(result, Ok(42.0));
		});
	}
}
