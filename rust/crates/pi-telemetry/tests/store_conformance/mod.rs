//! Port of `packages/telemetry/test/store-conformance.ts`: shared read-store checks; no raw-measurement or
//! backend-specific persistence assumptions. `telemetry_store_conformance!` registers the cases for one factory.
#![allow(dead_code)]

use std::future::ready;
use std::rc::Rc;

use pi_host::LocalBoxFuture;
use pi_host::testing::TestHost;
use pi_telemetry::{
	AttributeValue, InstrumentOptions, MetricKind, MetricQuery, SpanLink, SpanOptions, StoredSpanLink,
	TelemetryCarrier, TelemetryContext, TelemetryContextExt, TelemetryMetrics, TelemetryPropagation, TelemetryStore,
	TraceQuery,
};

use crate::attrs;
use crate::common::{options, set_now};

pub struct StoreFixture {
	pub context: Rc<dyn TelemetryContext>,
	pub metrics: Rc<dyn TelemetryMetrics>,
	pub propagation: Rc<dyn TelemetryPropagation>,
	pub store: Rc<dyn TelemetryStore>,
	pub flush: Rc<dyn Fn() -> LocalBoxFuture<'static, ()>>,
}

pub type StoreFixtureFactory = Rc<dyn Fn() -> LocalBoxFuture<'static, StoreFixture>>;

#[macro_export]
macro_rules! telemetry_store_conformance {
	($name:ident, $factory:expr) => {
		mod $name {
			use super::*;

			fn run<F, Fut>(test: F)
			where
				F: FnOnce(std::rc::Rc<pi_host::testing::TestHost>, $crate::store_conformance::StoreFixtureFactory) -> Fut,
				Fut: std::future::Future<Output = ()>,
			{
				pi_host::testing::run(|host| test(host, $factory));
			}

			#[test]
			fn returns_only_settled_traces_complete_span_snapshots_and_detached_results() {
				run($crate::store_conformance::returns_only_settled_traces_complete_span_snapshots_and_detached_results);
			}

			#[test]
			fn filters_by_first_span_name_and_start_any_error_and_all_attributes_on_one_span() {
				run($crate::store_conformance::filters_by_first_span_name_and_start_any_error_and_all_attributes_on_one_span);
			}

			#[test]
			fn pages_complete_traces_without_duplicates_when_trace_starts_tie() {
				run($crate::store_conformance::pages_complete_traces_without_duplicates_when_trace_starts_tie);
			}

			#[test]
			fn inherits_remote_trace_ids_and_preserves_remote_parents_and_cross_trace_links() {
				run($crate::store_conformance::inherits_remote_trace_ids_and_preserves_remote_parents_and_cross_trace_links);
			}

			#[test]
			fn accepts_external_string_identities_without_imposing_an_id_format() {
				run($crate::store_conformance::accepts_external_string_identities_without_imposing_an_id_format);
			}

			#[test]
			fn queries_metric_names_inclusive_interval_bounds_typed_attribute_equality_and_detached_values() {
				run($crate::store_conformance::queries_metric_names_inclusive_interval_bounds_typed_attribute_equality_and_detached_values);
			}
		}
	};
}

/// `/^[0-9a-f-]{36}:[1-9][0-9]*$/`.
fn is_memory_span_id(id: &str) -> bool {
	let Some((scope, span)) = id.split_once(':') else {
		return false;
	};
	scope.len() == 36
		&& scope
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
		&& span.bytes().next().is_some_and(|byte| (b'1'..=b'9').contains(&byte))
		&& span.bytes().all(|byte| byte.is_ascii_digit())
}

fn roundtrip(carrier: &TelemetryCarrier) -> TelemetryCarrier {
	serde_json::from_str(&pi_host::js::json_stringify(carrier)).unwrap()
}

pub async fn returns_only_settled_traces_complete_span_snapshots_and_detached_results(
	host: Rc<TestHost>,
	factory: StoreFixtureFactory,
) {
	set_now(&host, 20_000.0);
	let f = factory().await;
	let (resolve, gate) = futures::channel::oneshot::channel::<()>();
	let active = f.context.start_span(
		SpanOptions {
			name: "root".to_string(),
			attributes: Some(attrs! { "tags" => vec!["original"] }),
			links: None,
		},
		move |root| {
			root.add_event("event", Some(&attrs! { "tags" => vec!["original"] }));
			async move {
				let _ = gate.await;
				root.start_span(options("child"), |_| ready(Ok(()))).await
			}
		},
	);
	(f.flush)().await;
	assert_eq!(f.store.find_traces(None).await.unwrap().traces, vec![]);
	assert_eq!(f.store.get_trace("missing").await.unwrap(), None);
	resolve.send(()).unwrap();
	active.await.unwrap();
	(f.flush)().await;
	let trace = f.store.find_traces(None).await.unwrap().traces.remove(0);
	let mut root = trace.spans.iter().find(|span| span.name == "root").unwrap().clone();
	let child = trace.spans.iter().find(|span| span.name == "child").unwrap();
	assert!(is_memory_span_id(&root.id), "{}", root.id);
	assert_eq!(root.trace_id, root.id);
	assert_eq!(root.parent_id, None);
	assert_eq!(child.parent_id.as_ref(), Some(&root.id));
	assert_eq!(child.trace_id, root.trace_id);
	assert_eq!(root.started_ms, 20_000.0);
	assert!(root.ended_ms >= root.started_ms);
	assert!(root.events[0].time_ms >= 0.0);
	root.attributes.insert("tags".to_string(), Some(vec!["changed"].into()));
	root.events[0]
		.attributes
		.insert("tags".to_string(), Some(vec!["changed"].into()));
	let fresh = f.store.get_trace(&trace.trace_id).await.unwrap().unwrap();
	let fresh_root = fresh.spans.iter().find(|span| span.id == root.id).unwrap();
	assert_eq!(fresh_root.attributes.get("tags"), Some(&Some(vec!["original"].into())));
	assert_eq!(
		fresh_root.events[0].attributes.get("tags"),
		Some(&Some(vec!["original"].into()))
	);
}

pub async fn filters_by_first_span_name_and_start_any_error_and_all_attributes_on_one_span(
	host: Rc<TestHost>,
	factory: StoreFixtureFactory,
) {
	set_now(&host, 10_000.0);
	let f = factory().await;
	let clock = host.clone();
	f.context
		.start_span(options("old"), move |root| {
			set_now(&clock, 30_000.0);
			async move {
				root.start_span(
					SpanOptions {
						name: "recent-child".to_string(),
						attributes: Some(attrs! { "left" => 1.0 }),
						links: None,
					},
					|_| ready(Ok(())),
				)
				.await?;
				root.start_span(
					SpanOptions {
						name: "split".to_string(),
						attributes: Some(attrs! { "right" => 2.0 }),
						links: None,
					},
					|_| ready(Ok(())),
				)
				.await
			}
		})
		.await
		.unwrap();
	set_now(&host, 20_000.0);
	let clock = host.clone();
	f.context
		.start_span(options("wanted"), move |root| {
			set_now(&clock, 20_001.0);
			async move {
				root.start_span(
					SpanOptions {
						name: "failure".to_string(),
						attributes: Some(
							attrs! { "left" => 1.0, "right" => 2.0, "valid" => true, "odd.\"key" => "value" },
						),
						links: None,
					},
					|child| {
						child.set_status(&pi_telemetry::SpanStatus::Error { error: None });
						ready(Ok(()))
					},
				)
				.await
			}
		})
		.await
		.unwrap();
	set_now(&host, 40_000.0);
	f.context
		.start_span(
			SpanOptions {
				name: "new".to_string(),
				attributes: Some(attrs! { "valid" => 1.0 }),
				links: None,
			},
			|_| ready(Ok(())),
		)
		.await
		.unwrap();
	(f.flush)().await;
	let store = f.store.clone();
	let names = move |query: TraceQuery| {
		let store = store.clone();
		async move {
			store
				.find_traces(Some(query))
				.await
				.unwrap()
				.traces
				.into_iter()
				.map(|trace| trace.spans[0].name.clone())
				.collect::<Vec<_>>()
		}
	};
	let scalars = |entries: &[(&str, AttributeValue)]| {
		Some(
			entries
				.iter()
				.map(|(key, value)| (key.to_string(), value.clone()))
				.collect(),
		)
	};
	assert_eq!(
		names(TraceQuery {
			name: Some("wanted".to_string()),
			attributes: scalars(&[("left", 1.0.into()), ("right", 2.0.into())]),
			errors: Some(true),
			start_min: Some(20_000.0),
			start_max: Some(20_000.0),
			..TraceQuery::default()
		})
		.await,
		vec!["wanted"]
	);
	assert_eq!(
		names(TraceQuery {
			name: Some("failure".to_string()),
			..TraceQuery::default()
		})
		.await,
		Vec::<String>::new()
	);
	assert_eq!(
		names(TraceQuery {
			attributes: scalars(&[("left", 1.0.into()), ("right", 2.0.into())]),
			..TraceQuery::default()
		})
		.await,
		vec!["wanted"]
	);
	assert_eq!(
		names(TraceQuery {
			attributes: scalars(&[("valid", true.into()), ("odd.\"key", "value".into())]),
			..TraceQuery::default()
		})
		.await,
		vec!["wanted"]
	);
	assert_eq!(
		names(TraceQuery {
			attributes: scalars(&[("valid", 1.0.into())]),
			..TraceQuery::default()
		})
		.await,
		vec!["new"]
	);
	assert_eq!(
		names(TraceQuery {
			errors: Some(true),
			..TraceQuery::default()
		})
		.await,
		vec!["wanted"]
	);
	assert_eq!(
		names(TraceQuery {
			start_min: Some(20_000.0),
			..TraceQuery::default()
		})
		.await,
		vec!["new", "wanted"]
	);
	assert_eq!(
		names(TraceQuery {
			start_max: Some(10_000.0),
			..TraceQuery::default()
		})
		.await,
		vec!["old"]
	);
}

pub async fn pages_complete_traces_without_duplicates_when_trace_starts_tie(
	host: Rc<TestHost>,
	factory: StoreFixtureFactory,
) {
	set_now(&host, 20_000.0);
	let f = factory().await;
	for index in 0..25 {
		set_now(&host, 20_000.0);
		let clock = host.clone();
		f.context
			.start_span(
				SpanOptions {
					name: "root".to_string(),
					attributes: Some(attrs! { "index" => f64::from(index) }),
					links: None,
				},
				move |root| {
					set_now(&clock, 20_001.0);
					async move { root.start_span(options("child"), |_| ready(Ok(()))).await }
				},
			)
			.await
			.unwrap();
	}
	(f.flush)().await;
	let first = f.store.find_traces(None).await.unwrap();
	assert_eq!(first.traces.len(), 20);
	assert!(first.cursor.is_some());
	let rest = f
		.store
		.find_traces(Some(TraceQuery {
			cursor: first.cursor.clone(),
			..TraceQuery::default()
		}))
		.await
		.unwrap();
	assert_eq!(rest.traces.len(), 5);
	assert_eq!(rest.cursor, None);
	let all = f
		.store
		.find_traces(Some(TraceQuery {
			limit: Some(100.0),
			..TraceQuery::default()
		}))
		.await
		.unwrap()
		.traces;
	assert_eq!([first.traces.clone(), rest.traces.clone()].concat(), all);
	assert!(all.iter().all(|trace| trace.spans.len() == 2));
	let mut ids: Vec<String> = Vec::new();
	let mut cursor: Option<String> = None;
	loop {
		let page = f
			.store
			.find_traces(Some(TraceQuery {
				name: Some("root".to_string()),
				limit: Some(3.0),
				cursor: cursor.clone(),
				..TraceQuery::default()
			}))
			.await
			.unwrap();
		ids.extend(page.traces.iter().map(|trace| trace.trace_id.clone()));
		cursor = page.cursor;
		if cursor.is_none() {
			break;
		}
	}
	assert_eq!(ids, all.iter().map(|trace| trace.trace_id.clone()).collect::<Vec<_>>());
}

pub async fn inherits_remote_trace_ids_and_preserves_remote_parents_and_cross_trace_links(
	host: Rc<TestHost>,
	factory: StoreFixtureFactory,
) {
	set_now(&host, 20_000.0);
	let sender = factory().await;
	let receiver = factory().await;
	let carrier: Rc<std::cell::RefCell<Option<TelemetryCarrier>>> = Rc::default();
	let slot = carrier.clone();
	let propagation = sender.propagation.clone();
	sender
		.context
		.start_span(options("sender"), move |span| {
			*slot.borrow_mut() = propagation.inject(span.as_ref());
			ready(Ok(()))
		})
		.await
		.unwrap();
	let carrier = carrier.borrow().clone().unwrap();
	(sender.flush)().await;
	let sent = sender.store.find_traces(None).await.unwrap().traces.remove(0);
	let remote = receiver.propagation.extract(Some(&roundtrip(&carrier)));
	remote.start_span(options("receiver"), |_| ready(Ok(()))).await.unwrap();
	receiver
		.context
		.start_span(
			SpanOptions {
				name: "linked".to_string(),
				attributes: None,
				links: Some(vec![SpanLink {
					context: remote,
					attributes: Some(attrs! { "reason" => "background" }),
				}]),
			},
			|_| ready(Ok(())),
		)
		.await
		.unwrap();
	(receiver.flush)().await;
	let continued = receiver.store.get_trace(&sent.trace_id).await.unwrap().unwrap();
	assert_eq!(continued.spans.len(), 1);
	assert_eq!(continued.spans[0].trace_id, sent.trace_id);
	assert_eq!(continued.spans[0].parent_id.as_ref(), Some(&sent.spans[0].id));
	assert_eq!(continued.spans[0].name, "receiver");
	assert_ne!(continued.spans[0].id, sent.spans[0].id);
	let linked = receiver
		.store
		.find_traces(Some(TraceQuery {
			name: Some("linked".to_string()),
			..TraceQuery::default()
		}))
		.await
		.unwrap()
		.traces
		.remove(0);
	assert_ne!(linked.trace_id, sent.trace_id);
	assert_eq!(
		linked.spans[0].links,
		vec![StoredSpanLink {
			trace_id: sent.trace_id.clone(),
			span_id: sent.spans[0].id.clone(),
			attributes: Some(attrs! { "reason" => "background" }),
		}]
	);
	// The TS case also extracts carriers with a non-string value per key; `TelemetryCarrier` values are always
	// strings in Rust, so only the incomplete carriers are representable.
	for key in carrier.keys() {
		let mut incomplete = carrier.clone();
		incomplete.shift_remove(key);
		receiver
			.propagation
			.extract(Some(&incomplete))
			.start_span(options("fallback"), |_| ready(Ok(())))
			.await
			.unwrap();
	}
	(receiver.flush)().await;
	let fallbacks = receiver
		.store
		.find_traces(Some(TraceQuery {
			name: Some("fallback".to_string()),
			..TraceQuery::default()
		}))
		.await
		.unwrap()
		.traces;
	assert_eq!(fallbacks.len(), carrier.len());
	assert!(
		fallbacks
			.iter()
			.all(|trace| trace.spans[0].parent_id.is_none() && trace.trace_id == trace.spans[0].id)
	);
}

pub async fn accepts_external_string_identities_without_imposing_an_id_format(
	_host: Rc<TestHost>,
	factory: StoreFixtureFactory,
) {
	let f = factory().await;
	let carrier: Rc<std::cell::RefCell<Option<TelemetryCarrier>>> = Rc::default();
	let slot = carrier.clone();
	let propagation = f.propagation.clone();
	f.context
		.start_span(options("source"), move |span| {
			*slot.borrow_mut() = propagation.inject(span.as_ref());
			ready(Ok(()))
		})
		.await
		.unwrap();
	let carrier = carrier.borrow().clone().unwrap();
	let external: TelemetryCarrier = carrier
		.keys()
		.map(|key| (key.clone(), format!("external-{key}")))
		.collect();
	let parent = f.propagation.extract(Some(&external));
	assert_eq!(f.propagation.inject(parent.as_ref()), Some(external));
	parent
		.start_span(options("continued"), |_| ready(Ok(())))
		.await
		.unwrap();
	(f.flush)().await;
	let trace = f
		.store
		.find_traces(Some(TraceQuery {
			name: Some("continued".to_string()),
			..TraceQuery::default()
		}))
		.await
		.unwrap()
		.traces
		.remove(0);
	assert!(trace.trace_id.starts_with("external-"));
	assert!(trace.spans[0].parent_id.as_ref().unwrap().starts_with("external-"));
}

pub async fn queries_metric_names_inclusive_interval_bounds_typed_attribute_equality_and_detached_values(
	host: Rc<TestHost>,
	factory: StoreFixtureFactory,
) {
	set_now(&host, 20_001.0);
	let f = factory().await;
	let counter = f.metrics.counter(
		"hits",
		Some(&InstrumentOptions {
			description: None,
			unit: Some("By".to_string()),
		}),
	);
	counter.add(2.0, Some(&attrs! { "method" => "read", "valid" => true }));
	counter.add(3.0, Some(&attrs! { "valid" => true, "method" => "read" }));
	counter.add(11.0, Some(&attrs! { "method" => "write", "valid" => 1.0 }));
	f.metrics
		.counter("other", None)
		.add(100.0, Some(&attrs! { "method" => "read", "valid" => true }));
	set_now(&host, 30_001.0);
	counter.add(7.0, Some(&attrs! { "method" => "read", "valid" => true }));
	set_now(&host, 40_001.0);
	(f.flush)().await;
	let query = MetricQuery {
		name: "hits".to_string(),
		start_min: Some(20_000.0),
		start_max: Some(20_000.0),
		attributes: Some(
			[
				("method".to_string(), "read".into()),
				("valid".to_string(), true.into()),
			]
			.into_iter()
			.collect(),
		),
	};
	let mut rows = f.store.find_metrics(query.clone()).await.unwrap();
	assert_eq!(rows.iter().map(|row| row.count).sum::<f64>(), 2.0);
	assert_eq!(rows.iter().map(|row| row.sum).sum::<f64>(), 5.0);
	assert_eq!(rows.iter().map(|row| row.min).fold(f64::INFINITY, f64::min), 2.0);
	assert_eq!(rows.iter().map(|row| row.max).fold(f64::NEG_INFINITY, f64::max), 3.0);
	assert!(rows.iter().all(|row| row.kind == MetricKind::Counter
		&& row.unit.as_deref() == Some("By")
		&& row.interval_ms == 20_000.0));
	rows[0].attributes.insert("method".to_string(), Some("changed".into()));
	assert_eq!(
		f.store.find_metrics(query).await.unwrap()[0].attributes.get("method"),
		Some(&Some("read".into()))
	);
	assert_eq!(
		f.store
			.find_metrics(MetricQuery {
				name: "missing".to_string(),
				..MetricQuery::default()
			})
			.await
			.unwrap(),
		vec![]
	);
	assert_eq!(
		f.store
			.find_metrics(MetricQuery {
				name: "hits".to_string(),
				attributes: Some([("valid".to_string(), 1.0.into())].into_iter().collect()),
				..MetricQuery::default()
			})
			.await
			.unwrap()
			.iter()
			.map(|row| row.sum)
			.sum::<f64>(),
		11.0
	);
	assert_eq!(
		f.store
			.find_metrics(MetricQuery {
				name: "hits".to_string(),
				start_min: Some(30_000.0),
				..MetricQuery::default()
			})
			.await
			.unwrap()
			.iter()
			.map(|row| row.sum)
			.sum::<f64>(),
		7.0
	);
}
