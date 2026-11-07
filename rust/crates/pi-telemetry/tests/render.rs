//! Port of `packages/telemetry/test/render.test.ts`.

mod common;

use pi_telemetry::render::render_trace;
use pi_telemetry::{SpanStatus, SpanStatusError, StoredSpan, StoredSpanEvent, StoredSpanLink};

fn span(id: &str, overrides: impl FnOnce(&mut StoredSpan)) -> StoredSpan {
	let mut span = StoredSpan {
		id: id.to_string(),
		trace_id: "trace".to_string(),
		parent_id: None,
		name: id.to_string(),
		started_ms: 0.0,
		ended_ms: 10.0,
		attributes: attrs! {},
		events: Vec::new(),
		links: Vec::new(),
		status: SpanStatus::Ok,
	};
	overrides(&mut span);
	span
}

fn link(trace_id: &str, span_id: &str, attributes: Option<pi_telemetry::SpanAttributes>) -> StoredSpanLink {
	StoredSpanLink {
		trace_id: trace_id.to_string(),
		span_id: span_id.to_string(),
		attributes,
	}
}

mod render_trace {
	use super::*;

	#[test]
	fn renders_an_unordered_trace_with_attributes_durations_errors_events_and_links() {
		let root = span("root", |span| {
			span.started_ms = 1000.0;
			span.ended_ms = 1020.0;
			span.attributes = attrs! {
				"request" => "a b",
				"attempt" => 2.0,
				"cached" => false,
				"tags" => vec!["a", "b"],
				"sizes" => vec![1.0, 2.0],
				"flags" => vec![true, false],
			};
			span.events = vec![
				StoredSpanEvent {
					name: "checkpoint".to_string(),
					time_ms: 2.0,
					attributes: attrs! { "phase" => "tools" },
				},
				StoredSpanEvent {
					name: "tick".to_string(),
					time_ms: 3.5,
					attributes: attrs! {},
				},
			];
			span.links = vec![
				link("remote-trace", "remote", Some(attrs! { "reason" => "steer" })),
				link("other-trace", "other", None),
			];
		});
		let first = span("first", |span| {
			span.parent_id = Some("root".to_string());
			span.started_ms = 1005.0;
			span.ended_ms = 1009.5;
			span.status = SpanStatus::Error {
				error: Some(SpanStatusError {
					name: "Error".to_string(),
					message: "failed call".to_string(),
				}),
			};
		});
		let nested = span("nested", |span| {
			span.parent_id = Some("first".to_string());
			span.started_ms = 1006.0;
			span.ended_ms = 1007.0;
		});
		let second = span("second", |span| {
			span.parent_id = Some("root".to_string());
			span.started_ms = 1010.0;
			span.ended_ms = 1012.0;
			span.status = SpanStatus::Error { error: None };
		});
		let spans = vec![second, nested, first, root];
		let before = spans.clone();
		assert_eq!(
			render_trace(&spans),
			[
				r#"root request="a b" attempt=2 cached=false tags=["a","b"] sizes=[1,2] flags=[true,false] (20ms)"#,
				r#"  · checkpoint phase="tools" (+2ms)"#,
				"  · tick (+3.5ms)",
				r#"  → link trace remote-trace span remote reason="steer""#,
				"  → link trace other-trace span other",
				"  first (4.5ms) error Error: failed call",
				"    nested (1ms)",
				"  second (2ms) error",
			]
			.join("\n")
		);
		assert_eq!(spans, before);
	}

	#[test]
	fn treats_missing_parents_as_roots_and_orders_roots_and_children_by_start_time() {
		assert_eq!(
			render_trace(&[
				span("later", |span| {
					span.parent_id = Some("external".to_string());
					span.started_ms = 20.0;
					span.ended_ms = 30.0;
				}),
				span("child", |span| {
					span.parent_id = Some("later".to_string());
					span.started_ms = 25.0;
					span.ended_ms = 30.0;
				}),
				span("earlier", |span| {
					span.started_ms = 10.0;
					span.ended_ms = 15.0;
					span.links = vec![link("trace", "later", None)];
				}),
			]),
			[
				"earlier (5ms)",
				"  → link trace trace span later",
				"later (10ms)",
				"  child (5ms)"
			]
			.join("\n")
		);
	}

	#[test]
	fn prints_cross_trace_links_without_following_them_including_cycles() {
		assert_eq!(
			render_trace(&[
				span("server:1", |span| {
					span.name = "server".to_string();
					span.trace_id = "server-trace".to_string();
					span.links = vec![link("worker-trace", "worker:1", None)];
				}),
				span("worker:1", |span| {
					span.name = "worker".to_string();
					span.trace_id = "worker-trace".to_string();
					span.links = vec![link("server-trace", "server:1", None)];
				}),
			]),
			[
				"server (10ms)",
				"  → link trace worker-trace span worker:1",
				"worker (10ms)",
				"  → link trace server-trace span server:1",
			]
			.join("\n")
		);
	}

	#[test]
	fn deduplicates_spans_merged_from_stores_by_global_id() {
		let root = span("server:1", |span| span.name = "root".to_string());
		let child = span("worker:1", |span| {
			span.name = "child".to_string();
			span.parent_id = Some("server:1".to_string());
			span.events = vec![StoredSpanEvent {
				name: "done".to_string(),
				time_ms: 3.0,
				attributes: attrs! {},
			}];
		});
		let grandchild = span("worker:2", |span| {
			span.name = "grandchild".to_string();
			span.parent_id = Some("worker:1".to_string());
		});
		assert_eq!(
			render_trace(&[child.clone(), root.clone(), grandchild, root.clone(), child.clone()]),
			"root (10ms)\n  child (10ms)\n    · done (+3ms)\n    grandchild (10ms)"
		);
	}

	#[test]
	fn preserves_input_order_for_equal_start_times_without_mutating_the_input() {
		let spans = vec![
			span("root", |_| {}),
			span("z", |span| span.parent_id = Some("root".to_string())),
			span("a", |span| span.parent_id = Some("root".to_string())),
			span("other", |_| {}),
		];
		assert_eq!(
			render_trace(&spans),
			"root (10ms)\n  z (10ms)\n  a (10ms)\nother (10ms)"
		);
	}

	#[test]
	fn escapes_string_attribute_values_without_hiding_their_contents() {
		assert_eq!(
			render_trace(&[span("root", |span| {
				span.attributes = attrs! { "text" => "line one\n\"line two\"" };
			})]),
			r#"root text="line one\n\"line two\"" (10ms)"#
		);
	}

	#[test]
	fn renders_deep_trees_without_recursive_calls() {
		let spans: Vec<StoredSpan> = (0..2000)
			.map(|index| {
				span(&index.to_string(), |span| {
					if index != 0 {
						span.parent_id = Some((index - 1).to_string());
					}
				})
			})
			.collect();
		let rendered = render_trace(&spans);
		assert_eq!(rendered.split('\n').count(), spans.len());
		assert!(rendered.ends_with(&format!("{}1999 (10ms)", "  ".repeat(1999))));
	}

	#[test]
	fn renders_an_empty_input_as_an_empty_string() {
		assert_eq!(render_trace(&[]), "");
	}
}
