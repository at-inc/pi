//! Port of `packages/telemetry/test/propagation.test.ts` and `propagation-worker.ts`. The worker process
//! becomes a fresh OS thread with its own host and recorder; it shares no recorder state with the caller.

mod common;

use std::cell::RefCell;
use std::future::ready;
use std::rc::Rc;

use common::same_object;
use futures::FutureExt;
use pi_telemetry::{
	InMemoryTelemetryContext, NOOP_TELEMETRY_CONTEXT, NOOP_TELEMETRY_PROPAGATION, RecordedTelemetryLink,
	RecordedTelemetrySpan, SpanLink, SpanOptions, TelemetryCarrier, TelemetryContext, TelemetryContextExt,
	TelemetryPropagation, TelemetrySpan,
};

/// `propagation-worker.ts`: continues `carrier` in a fresh recorder and prints its spans as JSON.
fn propagation_worker(carrier: String) -> String {
	std::thread::spawn(move || {
		pi_host::testing::run(|_| async move {
			let telemetry = InMemoryTelemetryContext::new();
			let carrier: TelemetryCarrier = serde_json::from_str(&carrier).unwrap();
			telemetry
				.extract(Some(&carrier))
				.start_span(common::options("remote"), |_| ready(Ok(())))
				.await
				.unwrap();
			pi_host::js::json_stringify(&telemetry.get_spans())
		})
	})
	.join()
	.unwrap()
}

type SpanSlot = Rc<RefCell<Option<Rc<dyn TelemetrySpan>>>>;

mod propagation_and_links {
	use super::*;

	#[test]
	fn continues_a_parent_in_another_node_process_without_shared_recorder_state() {
		pi_host::testing::run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let carrier: Rc<RefCell<Option<TelemetryCarrier>>> = Rc::default();
			let slot = carrier.clone();
			let injector = telemetry.clone();
			telemetry
				.start_span(common::options("caller"), move |span| {
					*slot.borrow_mut() = injector.inject(span.as_ref());
					ready(Ok(()))
				})
				.await
				.unwrap();
			let carrier = carrier.borrow().clone();
			let output = propagation_worker(pi_host::js::json_stringify(&carrier));
			let recorded: Vec<RecordedTelemetrySpan> = serde_json::from_str(&output).unwrap();
			assert_eq!(recorded.len(), 1);
			assert_eq!(recorded[0].parent_context, carrier);
			assert_eq!(recorded[0].parent_id, None);
			assert_ne!(Some(recorded[0].context.clone()), carrier);
		});
	}

	#[test]
	fn isolates_crossed_concurrent_remote_parents_and_links_shared_roots() {
		pi_host::testing::run(|_| async {
			let caller = InMemoryTelemetryContext::new();
			let provider = InMemoryTelemetryContext::new();
			let activation: SpanSlot = Rc::default();
			let (finish, gate) = futures::channel::oneshot::channel::<()>();
			let finish_activation = Rc::new(RefCell::new(Some(finish)));
			let gate = Rc::new(RefCell::new(Some(gate)));
			type Shared = futures::future::Shared<pi_host::LocalBoxFuture<'static, pi_host::PiResult<()>>>;
			let shared: Rc<RefCell<Option<Shared>>> = Rc::default();
			let starts = ["first", "later"].map(|name| {
				let caller_ref = caller.clone();
				let provider = provider.clone();
				let activation = activation.clone();
				let finish_activation = finish_activation.clone();
				let gate = gate.clone();
				let shared = shared.clone();
				caller.start_span(common::options(name), move |trigger| {
					let parent = provider.extract(caller_ref.inject(trigger.as_ref()).as_ref());
					parent.start_span(common::options(&format!("{name}.method")), move |getter| {
						if shared.borrow().is_none() {
							let slot = activation.clone();
							let gate = gate.borrow_mut().take();
							let started = provider.start_span(
								SpanOptions {
									name: "activation".to_string(),
									attributes: None,
									links: Some(vec![SpanLink {
										context: getter.clone(),
										attributes: None,
									}]),
								},
								move |span| {
									*slot.borrow_mut() = Some(span);
									async move {
										if let Some(gate) = gate {
											let _ = gate.await;
										}
										Ok(())
									}
								},
							);
							*shared.borrow_mut() = Some(started.shared());
						}
						if let Some(activation) = activation.borrow().clone() {
							getter.add_link(&SpanLink {
								context: activation,
								attributes: None,
							});
						}
						if name == "later"
							&& let Some(finish) = finish_activation.borrow_mut().take()
						{
							let _ = finish.send(());
						}
						shared.borrow().clone().expect("shared activation exists")
					})
				})
			});
			for result in futures::future::join_all(starts).await {
				result.unwrap();
			}
			let spans = provider.get_spans();
			let root = spans.iter().find(|span| span.name == "activation").unwrap();
			assert_eq!(root.parent_context, None);
			let first = spans.iter().find(|span| span.name == "first.method").unwrap();
			let later = spans.iter().find(|span| span.name == "later.method").unwrap();
			assert_eq!(first.parent_context, Some(caller.get_spans()[0].context.clone()));
			assert_eq!(later.parent_context, Some(caller.get_spans()[1].context.clone()));
			assert_eq!(root.links[0].context, first.context);
			assert_eq!(later.links[0].context, root.context);
		});
	}

	#[test]
	fn detaches_carrier_and_link_snapshots_and_retains_settled_target_identities() {
		pi_host::testing::run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let target: SpanSlot = Rc::default();
			let slot = target.clone();
			telemetry
				.start_span(common::options("target"), move |span| {
					*slot.borrow_mut() = Some(span);
					ready(Ok(()))
				})
				.await
				.unwrap();
			let Some(target) = target.borrow().clone() else {
				panic!("Expected target");
			};
			let original = telemetry.inject(target.as_ref());
			let Some(mut injected) = telemetry.inject(target.as_ref()) else {
				panic!("Expected carrier");
			};
			injected.insert("pi-memory-span".to_string(), "999".to_string());
			assert_eq!(telemetry.inject(target.as_ref()), original);
			telemetry
				.start_span(
					SpanOptions {
						name: "linked".to_string(),
						attributes: None,
						links: Some(vec![SpanLink {
							context: target,
							attributes: Some(attrs! { "tags" => vec!["original"] }),
						}]),
					},
					|_| ready(Ok(())),
				)
				.await
				.unwrap();
			let mut spans = telemetry.get_spans();
			let [first, linked] = spans.as_mut_slice() else {
				panic!("Expected spans and link");
			};
			first.context.insert("pi-memory-span".to_string(), "123".to_string());
			linked.links[0]
				.context
				.insert("pi-memory-span".to_string(), "456".to_string());
			linked.links[0]
				.attributes
				.insert("tags".to_string(), Some(vec!["mutated"].into()));
			assert_eq!(
				telemetry.get_spans()[1].links,
				vec![RecordedTelemetryLink {
					context: original.unwrap(),
					attributes: attrs! { "tags" => vec!["original"] },
				}]
			);
		});
	}

	#[test]
	fn links_a_new_root_to_an_extracted_remote_target() {
		pi_host::testing::run(|_| async {
			let sender = InMemoryTelemetryContext::new();
			let receiver = InMemoryTelemetryContext::new();
			let carrier: Rc<RefCell<Option<TelemetryCarrier>>> = Rc::default();
			let slot = carrier.clone();
			let injector = sender.clone();
			sender
				.start_span(common::options("trigger"), move |span| {
					*slot.borrow_mut() = injector.inject(span.as_ref());
					ready(Ok(()))
				})
				.await
				.unwrap();
			let carrier = carrier.borrow().clone().unwrap();
			let wire: TelemetryCarrier = serde_json::from_str(&pi_host::js::json_stringify(&carrier)).unwrap();
			let target = receiver.extract(Some(&wire));
			let late_target = target.clone();
			receiver
				.start_span(
					SpanOptions {
						name: "background".to_string(),
						attributes: None,
						links: Some(vec![SpanLink {
							context: target,
							attributes: Some(attrs! { "reason" => "restart" }),
						}]),
					},
					move |span| {
						span.add_link(&SpanLink {
							context: late_target,
							attributes: None,
						});
						ready(Ok(()))
					},
				)
				.await
				.unwrap();
			let recorded = receiver.get_spans().remove(0);
			assert_eq!(recorded.parent_context, None);
			assert_eq!(
				recorded.links,
				vec![
					RecordedTelemetryLink {
						context: carrier.clone(),
						attributes: attrs! { "reason" => "restart" },
					},
					RecordedTelemetryLink {
						context: carrier,
						attributes: attrs! {},
					},
				]
			);
		});
	}

	#[test]
	fn uses_the_root_for_absent_and_unknown_carriers() {
		pi_host::testing::run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let carriers: Vec<Option<TelemetryCarrier>> = vec![
				None,
				Some(TelemetryCarrier::new()),
				Some(TelemetryCarrier::from([("unknown".to_string(), "format".to_string())])),
			];
			for carrier in &carriers {
				assert!(same_object(&telemetry.extract(carrier.as_ref()), &telemetry));
			}
			let noop = NOOP_TELEMETRY_CONTEXT.with(Rc::clone);
			let propagation = NOOP_TELEMETRY_PROPAGATION.with(Rc::clone);
			assert_eq!(propagation.inject(noop.as_ref()), None);
			assert!(same_object(
				&propagation.extract(Some(&TelemetryCarrier::from([(
					"unknown".to_string(),
					"format".to_string()
				)]))),
				&noop
			));
			noop.start_span(common::options("noop"), |span| {
				let context: Rc<dyn TelemetryContext> = span.clone();
				span.add_link(&SpanLink {
					context,
					attributes: None,
				});
				ready(Ok(()))
			})
			.await
			.unwrap();
		});
	}

	#[test]
	fn treats_recognized_carrier_identifiers_as_opaque_strings() {
		pi_host::testing::run(|_| async {
			let telemetry = InMemoryTelemetryContext::new();
			let carrier = TelemetryCarrier::from([
				("pi-memory-scope".to_string(), "external-recorder".to_string()),
				("pi-memory-span".to_string(), "external-span".to_string()),
				("pi-memory-trace".to_string(), "external-trace".to_string()),
			]);
			telemetry
				.extract(Some(&carrier))
				.start_span(common::options("receiver"), |_| ready(Ok(())))
				.await
				.unwrap();
			assert_eq!(telemetry.get_spans()[0].parent_context, Some(carrier));
		});
	}
}
