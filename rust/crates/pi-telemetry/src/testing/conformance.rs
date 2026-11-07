//! Port of `packages/telemetry/src/testing/conformance.ts`.
//! PORT-STATUS: reviewed
//!
//! `node:assert/strict` failures become `Err(PiError)` named `AssertionError`, so a case's `run()` rejects like the
//! TS promise does. Assertion message texts are not Node's (Node renders a diff of the inspected values).
//! `strictEqual` and `deepStrictEqual` compare plain data by value, with `Object.is` for numbers.
//!
//! `await using` becomes `using_completion`: a disposal error replaces the scope's result, and when the scope also
//! failed it becomes a `SuppressedError` whose `data` holds the disposal `error` and the `suppressed` scope error.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::future::{Future, ready};
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::js::json_stringify;
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};

use super::types::{TelemetryAdapterConformanceCase, TelemetryAdapterFixture, TelemetryAdapterFixtureFactory};
use crate::memory::{RecordedTelemetryEvent, RecordedTelemetryLink, RecordedTelemetrySpan};
use crate::metrics::{HistogramOptions, InstrumentOptions, MetricKind};
use crate::{
	AttributeValue, NOOP_TELEMETRY_CONTEXT, SpanAttributes, SpanLink, SpanOptions, SpanStatus, SpanStatusError,
	TelemetryCarrier, TelemetryContext, TelemetryContextExt, TelemetryMetrics, TelemetryPropagation, TelemetrySpan,
};

type ConformanceTest = Rc<dyn Fn(Rc<dyn TelemetryAdapterFixture>) -> LocalBoxFuture<'static, PiResult<()>>>;

fn create_case(
	factory: &TelemetryAdapterFixtureFactory,
	group: &str,
	name: &str,
	test: ConformanceTest,
) -> TelemetryAdapterConformanceCase {
	let factory = factory.clone();
	TelemetryAdapterConformanceCase::new(
		group,
		name,
		Rc::new(move || {
			let factory = factory.clone();
			let test = test.clone();
			Box::pin(async move {
				let fixture = factory().await?;
				let result = test(fixture.clone()).await;
				using_completion(result, fixture.async_dispose().await)
			})
		}),
	)
}

/// The completion of an `await using` scope: `completion` is the scope's result, `disposal` the dispose result.
fn using_completion(completion: PiResult<()>, disposal: PiResult<()>) -> PiResult<()> {
	match (completion, disposal) {
		(completion, Ok(())) => completion,
		(Ok(()), Err(error)) => Err(error),
		(Err(suppressed), Err(error)) => {
			Err(
				PiError::named("SuppressedError", "An error was suppressed during disposal.").with_data(json!({
					"error": error_value(&error),
					"suppressed": error_value(&suppressed),
				})),
			)
		}
	}
}

/// An error as a plain object: `name`, `message`, and the fields its `data` carries (a nested `SuppressedError`
/// keeps its `error` and `suppressed`).
fn error_value(error: &PiError) -> Value {
	let mut value = serde_json::Map::new();
	value.insert("name".to_string(), Value::String(error.name.clone()));
	value.insert("message".to_string(), Value::String(error.message.clone()));
	if let Some(Value::Object(data)) = &error.data {
		value.extend(data.clone());
	}
	Value::Object(value)
}

fn test<F, Fut>(test: F) -> ConformanceTest
where
	F: Fn(Rc<dyn TelemetryAdapterFixture>) -> Fut + 'static,
	Fut: Future<Output = PiResult<()>> + 'static,
{
	Rc::new(move |fixture| Box::pin(test(fixture)))
}

fn assertion_error(message: impl Into<String>) -> PiError {
	PiError::named("AssertionError", message)
}

fn ok(value: bool, message: &str) -> PiResult<()> {
	if value { Ok(()) } else { Err(assertion_error(message)) }
}

/// `Object.is` for numbers and value equality for everything else the cases compare.
trait SameValue {
	fn same_value(&self, other: &Self) -> bool;
}

impl SameValue for f64 {
	fn same_value(&self, other: &Self) -> bool {
		(self.is_nan() && other.is_nan()) || (self == other && self.is_sign_negative() == other.is_sign_negative())
	}
}

macro_rules! same_value_by_eq {
	($($type:ty),+) => {
		$(impl SameValue for $type {
			fn same_value(&self, other: &Self) -> bool {
				self == other
			}
		})+
	};
}

same_value_by_eq!(bool, i32, usize, str, String, MetricKind, SpanStatus, PiError);

impl<T: SameValue + ?Sized> SameValue for &T {
	fn same_value(&self, other: &Self) -> bool {
		(**self).same_value(*other)
	}
}

impl<T: SameValue> SameValue for Option<T> {
	fn same_value(&self, other: &Self) -> bool {
		match (self, other) {
			(Some(left), Some(right)) => left.same_value(right),
			(None, None) => true,
			_ => false,
		}
	}
}

impl<T: SameValue> SameValue for Vec<T> {
	fn same_value(&self, other: &Self) -> bool {
		self.len() == other.len() && self.iter().zip(other).all(|(left, right)| left.same_value(right))
	}
}

impl<A: SameValue, B: SameValue, C: SameValue> SameValue for (A, B, C) {
	fn same_value(&self, other: &Self) -> bool {
		self.0.same_value(&other.0) && self.1.same_value(&other.1) && self.2.same_value(&other.2)
	}
}

impl<V: SameValue> SameValue for IndexMap<String, V> {
	fn same_value(&self, other: &Self) -> bool {
		self.len() == other.len()
			&& self
				.iter()
				.all(|(key, value)| other.get(key).is_some_and(|other| value.same_value(other)))
	}
}

impl SameValue for AttributeValue {
	fn same_value(&self, other: &Self) -> bool {
		match (self, other) {
			(AttributeValue::Number(left), AttributeValue::Number(right)) => left.same_value(right),
			(AttributeValue::NumberArray(left), AttributeValue::NumberArray(right)) => left.same_value(right),
			(left, right) => left == right,
		}
	}
}

impl SameValue for RecordedTelemetryLink {
	fn same_value(&self, other: &Self) -> bool {
		self.context.same_value(&other.context) && self.attributes.same_value(&other.attributes)
	}
}

impl SameValue for RecordedTelemetryEvent {
	fn same_value(&self, other: &Self) -> bool {
		self.name == other.name && self.attributes.same_value(&other.attributes)
	}
}

fn strict_equal<T: SameValue + Debug + ?Sized>(actual: &T, expected: &T) -> PiResult<()> {
	if actual.same_value(expected) {
		return Ok(());
	}
	Err(assertion_error(format!(
		"Expected values to be strictly equal:\n\n{actual:?} !== {expected:?}\n"
	)))
}

fn deep_strict_equal<T: SameValue + Debug + ?Sized>(actual: &T, expected: &T) -> PiResult<()> {
	if actual.same_value(expected) {
		return Ok(());
	}
	Err(assertion_error(format!(
		"Expected values to be strictly deep-equal:\n{actual:?}\n\nshould equal\n\n{expected:?}"
	)))
}

/// `strictEqual` on object identity.
fn same<T: ?Sized>(actual: &Option<Rc<T>>, expected: &Option<Rc<T>>) -> PiResult<()> {
	let equal = match (actual, expected) {
		(Some(actual), Some(expected)) => Rc::ptr_eq(actual, expected),
		(None, None) => true,
		_ => false,
	};
	ok(equal, "Expected values to be strictly equal")
}

fn find_span(spans: &[RecordedTelemetrySpan], name: &str) -> PiResult<RecordedTelemetrySpan> {
	let span = spans.iter().find(|candidate| candidate.name == name);
	ok(span.is_some(), &format!("Expected recorded span {name}"))?;
	Ok(span.expect("checked above").clone())
}

async fn rejects_with_same_value<T>(
	operation: LocalBoxFuture<'static, PiResult<T>>,
	expected: &PiError,
) -> PiResult<()> {
	match operation.await {
		Ok(_) => strict_equal(&assertion_error("Expected operation to reject"), expected),
		Err(error) => strict_equal(&error, expected),
	}
}

fn options(name: &str) -> SpanOptions {
	SpanOptions {
		name: name.to_string(),
		..SpanOptions::default()
	}
}

fn attributes<const N: usize>(entries: [(&str, AttributeValue); N]) -> SpanAttributes {
	entries
		.into_iter()
		.map(|(name, value)| (name.to_string(), Some(value)))
		.collect()
}

fn roundtrip(carrier: &TelemetryCarrier) -> PiResult<TelemetryCarrier> {
	Ok(serde_json::from_str(&json_stringify(carrier))?)
}

type Capabilities = Rc<(Option<Rc<dyn TelemetryMetrics>>, Option<Rc<dyn TelemetryPropagation>>)>;

fn assert_capabilities(context: &dyn TelemetryContext, expected: &Capabilities) -> PiResult<()> {
	same(&context.metrics(), &expected.0)?;
	same(&context.propagation(), &expected.1)
}

type SpanSlot = Rc<RefCell<Option<Rc<dyn TelemetrySpan>>>>;

/// Creates runner-independent cases for the callback telemetry adapter contract.
pub fn create_telemetry_adapter_conformance(
	factory: TelemetryAdapterFixtureFactory,
) -> Vec<TelemetryAdapterConformanceCase> {
	let hop_factory = factory.clone();
	vec![
		create_case(
			&factory,
			"capabilities",
			"carries adapter capabilities through contexts and spans",
			test(|fixture| async move {
				let metrics = fixture.metrics().or_else(|| fixture.context().metrics());
				let propagation = fixture.propagation().or_else(|| fixture.context().propagation());
				let expected: Capabilities = Rc::new((metrics, propagation));
				assert_capabilities(fixture.context().as_ref(), &expected)?;
				let settled: SpanSlot = Rc::default();
				let slot = settled.clone();
				let inner = expected.clone();
				fixture
					.context()
					.start_span(options("capabilities"), move |span| {
						*slot.borrow_mut() = Some(span.clone());
						async move {
							assert_capabilities(span.as_ref(), &inner)?;
							let child_expected = inner.clone();
							span.start_span(options("capabilities.child"), move |child| {
								ready(assert_capabilities(child.as_ref(), &child_expected))
							})
							.await?;
							if let Some(propagation) = &inner.1 {
								let parent = propagation.extract(propagation.inject(span.as_ref()).as_ref());
								assert_capabilities(parent.as_ref(), &inner)?;
								let child_expected = inner.clone();
								parent
									.start_span(options("capabilities.extracted"), move |child| {
										ready(assert_capabilities(child.as_ref(), &child_expected))
									})
									.await?;
							}
							Ok(())
						}
					})
					.await?;
				let settled = settled.borrow().clone();
				ok(settled.is_some(), "Expected the callback span")?;
				let settled = settled.expect("checked above");
				settled
					.start_span(options("capabilities.inert"), move |child| {
						ready(assert_capabilities(child.as_ref(), &expected))
					})
					.await
			}),
		),
		create_case(
			&factory,
			"metrics",
			"records ordered measurements and optional histogram exemplars",
			test(|fixture| async move {
				let Some(metrics) = fixture.metrics() else {
					return Ok(());
				};
				let get_metrics = fixture.get_metrics();
				ok(
					get_metrics.is_some(),
					"Metrics fixtures require a normalized measurement reader",
				)?;
				let get_metrics = get_metrics.expect("checked above");
				metrics
					.counter(
						"bytes",
						Some(&InstrumentOptions {
							description: Some("Encoded bytes".to_string()),
							unit: Some("By".to_string()),
						}),
					)
					.add(12.0, Some(&attributes([("direction", "out".into())])));
				let active = metrics.up_down_counter("active", None);
				active.add(1.0, None);
				active.add(-1.0, None);
				let histogram = metrics.histogram(
					"duration",
					Some(&HistogramOptions {
						base: InstrumentOptions {
							description: None,
							unit: Some("ms".to_string()),
						},
						boundaries: Some(vec![1.0, 10.0]),
					}),
				);
				fixture
					.context()
					.start_span(options("measured"), move |span| {
						histogram.record(5.0, Some(&attributes([("method", "read".into())])), Some(span.as_ref()));
						ready(Ok(()))
					})
					.await?;
				let measurements = get_metrics().await?;
				deep_strict_equal(
					&measurements
						.iter()
						.map(|measurement| (measurement.kind, measurement.name.clone(), measurement.value))
						.collect::<Vec<_>>(),
					&vec![
						(MetricKind::Counter, "bytes".to_string(), 12.0),
						(MetricKind::UpDownCounter, "active".to_string(), 1.0),
						(MetricKind::UpDownCounter, "active".to_string(), -1.0),
						(MetricKind::Histogram, "duration".to_string(), 5.0),
					],
				)?;
				deep_strict_equal(
					&measurements.first().map(|measurement| measurement.attributes.clone()),
					&Some(attributes([("direction", "out".into())])),
				)?;
				strict_equal(
					&measurements
						.first()
						.and_then(|measurement| measurement.description.clone()),
					&Some("Encoded bytes".to_string()),
				)?;
				strict_equal(
					&measurements.first().and_then(|measurement| measurement.unit.clone()),
					&Some("By".to_string()),
				)?;
				deep_strict_equal(
					&measurements
						.get(3)
						.and_then(|measurement| measurement.boundaries.clone()),
					&Some(vec![1.0, 10.0]),
				)?;
				let measured = find_span(&fixture.get_spans().await?, "measured")?;
				deep_strict_equal(
					&measurements.get(3).and_then(|measurement| measurement.context.clone()),
					&Some(measured.context),
				)
			}),
		),
		create_case(
			&factory,
			"metrics",
			"copies instrument metadata and measurement attributes",
			test(|fixture| async move {
				let Some(metrics) = fixture.metrics() else {
					return Ok(());
				};
				let get_metrics = fixture.get_metrics();
				ok(
					get_metrics.is_some(),
					"Metrics fixtures require a normalized measurement reader",
				)?;
				let get_metrics = get_metrics.expect("checked above");
				let mut histogram_options = HistogramOptions {
					base: InstrumentOptions {
						description: Some("Original".to_string()),
						unit: None,
					},
					boundaries: Some(vec![1.0, 10.0]),
				};
				let histogram = metrics.histogram("snapshot", Some(&histogram_options));
				if let Some(boundaries) = &mut histogram_options.boundaries {
					boundaries.push(100.0);
				}
				histogram_options.description = Some("Changed".to_string());
				let mut recorded_attributes = attributes([("tags", vec!["original"].into())]);
				histogram.record(-1.0, Some(&recorded_attributes), None);
				if let Some(Some(AttributeValue::StringArray(tags))) = recorded_attributes.get_mut("tags") {
					tags.push("changed".to_string());
				}
				let measurements = get_metrics().await?;
				strict_equal(&measurements.len(), &1)?;
				deep_strict_equal(
					&measurements.first().map(|measurement| measurement.attributes.clone()),
					&Some(attributes([("tags", vec!["original"].into())])),
				)?;
				deep_strict_equal(
					&measurements
						.first()
						.and_then(|measurement| measurement.boundaries.clone()),
					&Some(vec![1.0, 10.0]),
				)?;
				strict_equal(
					&measurements
						.first()
						.and_then(|measurement| measurement.description.clone()),
					&Some("Original".to_string()),
				)?;
				strict_equal(
					&measurements.first().map(|measurement| measurement.context.clone()),
					&Some(None),
				)
			}),
		),
		create_case(
			&factory,
			"links",
			"records initial and late links without changing parents",
			test(|fixture| async move {
				let target: SpanSlot = Rc::default();
				let slot = target.clone();
				fixture
					.context()
					.start_span(options("target"), move |span| {
						*slot.borrow_mut() = Some(span);
						ready(Ok(()))
					})
					.await?;
				let Some(target) = target.borrow().clone() else {
					return Err(PiError::new("Expected target span"));
				};
				let target_span = target.clone();
				let mut link_attributes = attributes([("reason", vec!["initial"].into())]);
				let linked_options = SpanOptions {
					name: "linked".to_string(),
					attributes: None,
					links: Some(vec![SpanLink {
						context: target,
						attributes: Some(link_attributes.clone()),
					}]),
				};
				fixture
					.context()
					.start_span(linked_options, move |span| {
						if let Some(Some(AttributeValue::StringArray(reason))) = link_attributes.get_mut("reason") {
							reason.push("mutated".to_string());
						}
						span.add_link(&SpanLink {
							context: target_span,
							attributes: Some(attributes([("reason", "late".into())])),
						});
						span.add_link(&SpanLink {
							context: NOOP_TELEMETRY_CONTEXT.with(Rc::clone),
							attributes: None,
						});
						ready(Ok(()))
					})
					.await?;
				let spans = fixture.get_spans().await?;
				let linked = find_span(&spans, "linked")?;
				strict_equal(&linked.parent_id, &None)?;
				strict_equal(&linked.parent_context, &None)?;
				let target_context = find_span(&spans, "target")?.context;
				deep_strict_equal(
					&linked.links,
					&vec![
						RecordedTelemetryLink {
							context: target_context.clone(),
							attributes: attributes([("reason", vec!["initial"].into())]),
						},
						RecordedTelemetryLink {
							context: target_context,
							attributes: attributes([("reason", "late".into())]),
						},
					],
				)
			}),
		),
		create_case(
			&factory,
			"links",
			"ignores links added after source settlement",
			test(|fixture| async move {
				let source: SpanSlot = Rc::default();
				let slot = source.clone();
				fixture
					.context()
					.start_span(options("link-source"), move |span| {
						*slot.borrow_mut() = Some(span);
						ready(Ok(()))
					})
					.await?;
				let Some(source) = source.borrow().clone() else {
					return Err(PiError::new("Expected source span"));
				};
				source.add_link(&SpanLink {
					context: source.clone(),
					attributes: None,
				});
				deep_strict_equal(
					&find_span(&fixture.get_spans().await?, "link-source")?.links,
					&Vec::new(),
				)
			}),
		),
		create_case(
			&factory,
			"propagation",
			"roundtrips parent identity through JSON without retaining the carrier",
			test(|fixture| async move {
				let Some(propagation) = fixture.propagation() else {
					return Ok(());
				};
				fixture
					.context()
					.start_span(options("sender"), move |span| async move {
						let carrier = propagation.inject(span.as_ref());
						ok(carrier.is_some(), "Expected a carrier")?;
						let carrier = carrier.expect("checked above");
						let mut wire = roundtrip(&carrier)?;
						let parent = propagation.extract(Some(&wire));
						for value in wire.values_mut() {
							*value = "mutated".to_string();
						}
						parent.start_span(options("receiver"), |_| ready(Ok(42.0))).await?;
						deep_strict_equal(&propagation.inject(span.as_ref()), &Some(carrier))
					})
					.await?;
				let spans = fixture.get_spans().await?;
				deep_strict_equal(
					&find_span(&spans, "receiver")?.parent_context,
					&Some(find_span(&spans, "sender")?.context),
				)
			}),
		),
		create_case(
			&factory,
			"propagation",
			"absent and unknown carriers produce recorded roots",
			test(|fixture| async move {
				let Some(propagation) = fixture.propagation() else {
					return Ok(());
				};
				strict_equal(
					&propagation.inject(NOOP_TELEMETRY_CONTEXT.with(Rc::clone).as_ref()),
					&None,
				)?;
				let carriers: Vec<Option<TelemetryCarrier>> = vec![
					None,
					Some(TelemetryCarrier::new()),
					Some(TelemetryCarrier::from([("unknown".to_string(), "format".to_string())])),
				];
				for (index, carrier) in carriers.iter().enumerate() {
					let context = propagation.extract(carrier.as_ref());
					strict_equal(
						&context
							.start_span(options(&format!("fallback-{index}")), |_| ready(Ok(42.0)))
							.await?,
						&42.0,
					)?;
				}
				for span in fixture.get_spans().await? {
					strict_equal(&span.parent_context, &None)?;
				}
				Ok(())
			}),
		),
		create_case(
			&factory,
			"propagation",
			"propagates across fresh adapters and multiple hops",
			test(move |fixture| {
				let factory = hop_factory.clone();
				async move {
					if fixture.propagation().is_none() {
						return Ok(());
					}
					let receiver = factory().await?;
					let worker = match factory().await {
						Ok(worker) => worker,
						Err(error) => return using_completion(Err(error), receiver.async_dispose().await),
					};
					let result = propagate_across_hops(fixture, receiver.clone(), worker.clone()).await;
					let result = using_completion(result, worker.async_dispose().await);
					using_completion(result, receiver.async_dispose().await)
				}
			}),
		),
		create_case(
			&factory,
			"callback lifecycle",
			"admits once synchronously and preserves the result",
			test(|fixture| async move {
				let admitted = Rc::new(Cell::new(false));
				let calls = Rc::new(Cell::new(0));
				let expected = Rc::new(json!({ "value": 42 }));
				let result = {
					let admitted = admitted.clone();
					let calls = calls.clone();
					let expected = expected.clone();
					fixture.context().start_span(options("success"), move |_| {
						admitted.set(true);
						calls.set(calls.get() + 1);
						ready(Ok(expected))
					})
				};

				strict_equal(&admitted.get(), &true)?;
				strict_equal(&calls.get(), &1)?;
				ok(
					Rc::ptr_eq(&result.await?, &expected),
					"Expected values to be strictly equal",
				)?;
				deep_strict_equal(
					&find_span(&fixture.get_spans().await?, "success")?.status,
					&SpanStatus::Ok,
				)?;
				strict_equal(&find_span(&fixture.get_spans().await?, "success")?.settled, &true)
			}),
		),
		create_case(
			&factory,
			"callback lifecycle",
			"preserves synchronous and asynchronous rejection values",
			test(|fixture| async move {
				let sync_error = PiError::new("sync");
				let thrown = sync_error.clone();
				rejects_with_same_value(
					fixture
						.context()
						.start_span(options("sync-error"), move |_| ready(Err::<(), _>(thrown))),
					&sync_error,
				)
				.await?;

				let async_error = PiError::named("", "[object Object]").with_data(json!({ "kind": "async" }));
				let thrown = async_error.clone();
				rejects_with_same_value(
					fixture
						.context()
						.start_span(options("async-error"), move |_| async move { Err::<(), _>(thrown) }),
					&async_error,
				)
				.await?;

				let undefined_error = PiError::named("", "undefined");
				let thrown = undefined_error.clone();
				rejects_with_same_value(
					fixture
						.context()
						.start_span(options("undefined-error"), move |_| ready(Err::<(), _>(thrown))),
					&undefined_error,
				)
				.await?;

				let spans = fixture.get_spans().await?;
				for name in ["sync-error", "async-error", "undefined-error"] {
					ok(
						matches!(find_span(&spans, name)?.status, SpanStatus::Error { .. }),
						"Expected values to be strictly equal",
					)?;
				}
				Ok(())
			}),
		),
		create_case(
			&factory,
			"status",
			"uses last explicit status without automatic overwrite",
			test(|fixture| async move {
				let expected = |message: &str| SpanStatus::Error {
					error: Some(SpanStatusError {
						name: "Expected".to_string(),
						message: message.to_string(),
					}),
				};
				let first = expected("first");
				fixture
					.context()
					.start_span(options("last-status"), move |span| {
						span.set_status(&first);
						span.set_status(&SpanStatus::Ok);
						ready(Ok(()))
					})
					.await?;

				let thrown = PiError::new("after explicit status");
				let rethrown = thrown.clone();
				rejects_with_same_value(
					fixture
						.context()
						.start_span(options("explicit-before-throw"), move |span| {
							span.set_status(&SpanStatus::Ok);
							ready(Err::<(), _>(rethrown))
						}),
					&thrown,
				)
				.await?;

				let rejected = PiError::new("after async explicit status");
				let rerejected = rejected.clone();
				let async_failure = expected("async failure");
				rejects_with_same_value(
					fixture
						.context()
						.start_span(options("explicit-before-rejection"), move |span| {
							span.set_status(&async_failure);
							ready(Err::<(), _>(rerejected))
						}),
					&rejected,
				)
				.await?;

				let returned_failure = expected("returned failure");
				fixture
					.context()
					.start_span(options("expected-failure"), move |span| {
						span.set_status(&returned_failure);
						ready(Ok(json!({ "ok": false })))
					})
					.await?;

				let spans = fixture.get_spans().await?;
				deep_strict_equal(&find_span(&spans, "last-status")?.status, &SpanStatus::Ok)?;
				deep_strict_equal(&find_span(&spans, "explicit-before-throw")?.status, &SpanStatus::Ok)?;
				deep_strict_equal(
					&find_span(&spans, "explicit-before-rejection")?.status,
					&expected("async failure"),
				)?;
				deep_strict_equal(
					&find_span(&spans, "expected-failure")?.status,
					&expected("returned failure"),
				)
			}),
		),
		create_case(
			&factory,
			"recording",
			"merges attributes and records ordered events",
			test(|fixture| async move {
				let mut start = attributes([("start", "value".into()), ("overwrite", "start".into())]);
				start.insert("ignored".to_string(), None);
				fixture
					.context()
					.start_span(
						SpanOptions {
							name: "recording".to_string(),
							attributes: Some(start),
							links: None,
						},
						|span| {
							span.set_attributes(&attributes([("count", 1.0.into()), ("overwrite", "middle".into())]));
							let mut end = SpanAttributes::new();
							end.insert("count".to_string(), None);
							end.insert("overwrite".to_string(), Some("end".into()));
							span.set_attributes(&end);
							let mut first = attributes([("index", 1.0.into())]);
							first.insert("ignored".to_string(), None);
							span.add_event("first", Some(&first));
							span.add_event("second", Some(&attributes([("index", 2.0.into())])));
							ready(Ok(()))
						},
					)
					.await?;

				let span = find_span(&fixture.get_spans().await?, "recording")?;
				deep_strict_equal(
					&span.attributes,
					&attributes([
						("start", "value".into()),
						("overwrite", "end".into()),
						("count", 1.0.into()),
					]),
				)?;
				deep_strict_equal(
					&span.events,
					&vec![
						RecordedTelemetryEvent {
							name: "first".to_string(),
							attributes: attributes([("index", 1.0.into())]),
						},
						RecordedTelemetryEvent {
							name: "second".to_string(),
							attributes: attributes([("index", 2.0.into())]),
						},
					],
				)
			}),
		),
		create_case(
			&factory,
			"recording",
			"makes calls after settlement inert",
			test(|fixture| async move {
				let settled_span: SpanSlot = Rc::default();
				let slot = settled_span.clone();
				fixture
					.context()
					.start_span(
						SpanOptions {
							name: "settled".to_string(),
							attributes: Some(attributes([("value", "initial".into())])),
							links: None,
						},
						move |span| {
							*slot.borrow_mut() = Some(span);
							ready(Ok(()))
						},
					)
					.await?;
				let Some(captured_span) = settled_span.borrow().clone() else {
					return Err(PiError::new("Expected callback span"));
				};

				captured_span.set_attributes(&attributes([("value", "late".into())]));
				captured_span.add_event("late", Some(&attributes([("value", true.into())])));
				captured_span.set_status(&SpanStatus::Error { error: None });
				let child_admitted = Rc::new(Cell::new(false));
				let admitted = child_admitted.clone();
				let child_result = captured_span.start_span(options("late-child"), move |_| {
					admitted.set(true);
					ready(Ok(7.0))
				});
				strict_equal(&child_admitted.get(), &true)?;
				strict_equal(&child_result.await?, &7.0)?;

				let spans = fixture.get_spans().await?;
				strict_equal(&spans.len(), &1)?;
				deep_strict_equal(
					&spans.first().map(|span| span.attributes.clone()),
					&Some(attributes([("value", "initial".into())])),
				)?;
				deep_strict_equal(&spans.first().map(|span| span.events.clone()), &Some(Vec::new()))?;
				deep_strict_equal(&spans.first().map(|span| span.status.clone()), &Some(SpanStatus::Ok))
			}),
		),
		create_case(
			&factory,
			"parentage",
			"records nested and concurrent child relationships",
			test(|fixture| async move {
				let (release_first, first_gate) = futures::channel::oneshot::channel::<()>();

				fixture
					.context()
					.start_span(options("parent"), move |parent| async move {
						let first = parent.start_span(options("first-child"), move |_| async move {
							let _ = first_gate.await;
							Ok(())
						});
						let second = parent.start_span(options("second-child"), |_| ready(Ok("done")));
						futures::future::try_join(first, async move {
							strict_equal(&second.await?, &"done")?;
							let _ = release_first.send(());
							Ok(())
						})
						.await
						.map(|_| ())
					})
					.await?;

				let spans = fixture.get_spans().await?;
				let parent = find_span(&spans, "parent")?;
				let first = find_span(&spans, "first-child")?;
				let second = find_span(&spans, "second-child")?;
				strict_equal(&parent.parent_id, &None)?;
				strict_equal(&first.parent_id, &Some(parent.id))?;
				strict_equal(&second.parent_id, &Some(parent.id))?;
				let (Some(second_end), Some(first_end), Some(parent_end)) =
					(second.end_sequence, first.end_sequence, parent.end_sequence)
				else {
					return Err(assertion_error("Expected end sequences"));
				};
				ok(second_end < first_end, "Expected second-child to settle first")?;
				ok(first_end < parent_end, "Expected first-child to settle before parent")
			}),
		),
	]
}

async fn propagate_across_hops(
	fixture: Rc<dyn TelemetryAdapterFixture>,
	receiver: Rc<dyn TelemetryAdapterFixture>,
	worker: Rc<dyn TelemetryAdapterFixture>,
) -> PiResult<()> {
	let (Some(receiver_propagation), Some(worker_propagation)) = (receiver.propagation(), worker.propagation()) else {
		return Err(PiError::new("Expected propagation on each fixture"));
	};
	let sender_carrier: Rc<RefCell<Option<TelemetryCarrier>>> = Rc::default();
	let slot = sender_carrier.clone();
	let sender = fixture.clone();
	fixture
		.context()
		.start_span(options("sender"), move |span| {
			*slot.borrow_mut() = sender
				.propagation()
				.and_then(|propagation| propagation.inject(span.as_ref()));
			ready(Ok(()))
		})
		.await?;
	let sender_carrier = sender_carrier.borrow().clone();
	ok(sender_carrier.is_some(), "Expected a sender carrier")?;
	let sender_carrier = sender_carrier.expect("checked above");

	let receiver_carrier: Rc<RefCell<Option<TelemetryCarrier>>> = Rc::default();
	let slot = receiver_carrier.clone();
	let inject = receiver.clone();
	receiver_propagation
		.extract(Some(&roundtrip(&sender_carrier)?))
		.start_span(options("receiver"), move |span| {
			*slot.borrow_mut() = inject
				.propagation()
				.and_then(|propagation| propagation.inject(span.as_ref()));
			ready(Ok(()))
		})
		.await?;
	let receiver_carrier = receiver_carrier.borrow().clone();
	ok(receiver_carrier.is_some(), "Expected a receiver carrier")?;
	let receiver_carrier = receiver_carrier.expect("checked above");

	worker_propagation
		.extract(Some(&roundtrip(&receiver_carrier)?))
		.start_span(options("worker"), |_| ready(Ok(())))
		.await?;
	deep_strict_equal(
		&find_span(&receiver.get_spans().await?, "receiver")?.parent_context,
		&Some(sender_carrier),
	)?;
	deep_strict_equal(
		&find_span(&worker.get_spans().await?, "worker")?.parent_context,
		&Some(receiver_carrier),
	)
}
