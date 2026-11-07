//! Port of `packages/ai/test/event-stream.test.ts`.
//!
//! TS iterators become `EventStream` clones polled with `next()`; `for await` drains with `while let`. Timing tests run
//! on `pi_host::testing`, whose `sleep` advances the virtual clock, so measured durations are exact.

use std::cell::RefCell;
use std::rc::Rc;

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;
use pi_ai::types::{AssistantMessage, AssistantMessageEvent, SharedAssistantMessage, StopReason, Usage};
use pi_ai::utils::event_stream::{AssistantMessageEventStream, EventStream};

// Regression tests for https://github.com/earendil-works/pi/issues/9055
mod event_stream {
	use super::*;

	#[test]
	fn drains_buffered_events_in_order_and_ignores_events_pushed_after_completion() {
		futures::executor::block_on(async {
			let stream: EventStream<i32, i32> = EventStream::new(|event| *event == 3, |event| *event);
			stream.push(1);
			stream.push(2);
			stream.push(3);
			stream.push(4);

			assert_eq!(stream.result().await, Some(3));

			let mut events = Vec::new();
			while let Some(event) = stream.next().await {
				events.push(event);
			}
			assert_eq!(events, vec![1, 2, 3]);
		});
	}

	#[test]
	fn preserves_order_when_events_arrive_after_buffered_draining_starts() {
		futures::executor::block_on(async {
			let stream: EventStream<i32, i32> = EventStream::new(|_| false, |event| *event);
			stream.push(1);
			stream.push(2);

			let iterator = stream.clone();
			assert_eq!(iterator.next().await, Some(1));

			stream.push(3);
			assert_eq!(iterator.next().await, Some(2));
			assert_eq!(iterator.next().await, Some(3));

			stream.end(Some(3));
			assert_eq!(iterator.next().await, None);
		});
	}

	#[test]
	fn delivers_events_to_waiting_consumers_in_registration_order() {
		let mut pool = LocalPool::new();
		let stream: EventStream<i32, i32> = EventStream::new(|_| false, |event| *event);
		let first_iterator = stream.clone();
		let second_iterator = stream.clone();
		let first_event = pool
			.spawner()
			.spawn_local_with_handle(async move { first_iterator.next().await })
			.unwrap();
		pool.run_until_stalled();
		let second_event = pool
			.spawner()
			.spawn_local_with_handle(async move { second_iterator.next().await })
			.unwrap();
		pool.run_until_stalled();

		stream.push(1);
		stream.push(2);

		assert_eq!(pool.run_until(first_event), Some(1));
		assert_eq!(pool.run_until(second_event), Some(2));
	}

	#[test]
	fn drains_buffered_events_after_end_and_resolves_the_explicit_result() {
		futures::executor::block_on(async {
			let stream: EventStream<i32, String> = EventStream::new(|_| false, |event: &i32| event.to_string());
			stream.push(1);
			stream.push(2);
			stream.end(Some("complete".to_string()));

			assert_eq!(stream.result().await, Some("complete".to_string()));

			let mut events = Vec::new();
			while let Some(event) = stream.next().await {
				events.push(event);
			}
			assert_eq!(events, vec![1, 2]);
		});
	}

	#[test]
	fn wakes_all_waiting_consumers_when_ended_without_a_result() {
		let mut pool = LocalPool::new();
		let stream: EventStream<i32, i32> = EventStream::new(|_| false, |event| *event);
		let first_iterator = stream.clone();
		let second_iterator = stream.clone();
		let first_event = pool
			.spawner()
			.spawn_local_with_handle(async move { first_iterator.next().await })
			.unwrap();
		let second_event = pool
			.spawner()
			.spawn_local_with_handle(async move { second_iterator.next().await })
			.unwrap();
		pool.run_until_stalled();

		stream.end(None);

		assert_eq!(pool.run_until(first_event), None);
		assert_eq!(pool.run_until(second_event), None);
	}
}

fn message(timestamp: f64, duration_ms: Option<f64>) -> AssistantMessage {
	AssistantMessage {
		content: vec![],
		api: "openai-responses".into(),
		provider: "openai".into(),
		model: "m".into(),
		response_model: None,
		response_id: None,
		provider_thinking_level: None,
		thinking_level: None,
		diagnostics: None,
		usage: Usage::default(),
		stop_reason: StopReason::Stop,
		deferred: None,
		error_message: None,
		raw_stop_reason: None,
		end_turn: None,
		timestamp,
		duration_ms,
	}
}

fn shared(message: AssistantMessage) -> SharedAssistantMessage {
	Rc::new(RefCell::new(message))
}

fn done(message: &SharedAssistantMessage) -> AssistantMessageEvent {
	AssistantMessageEvent::Done {
		reason: StopReason::Stop,
		message: message.clone(),
	}
}

async fn sleep(ms: f64) {
	pi_host::sleep(ms, None).await.unwrap();
}

mod assistant_message_event_stream_timing {
	use super::*;

	#[test]
	fn sets_duration_ms_on_the_final_done_or_error_message_of_a_response_it_saw_start() {
		pi_host::testing::run(|_host| async {
			let done_stream = AssistantMessageEventStream::new();
			let answer = shared(message(pi_host::now_ms(), None));
			sleep(20.0).await;
			done_stream.push(done(&answer));
			let measured = answer.borrow().duration_ms;
			assert!(measured.unwrap() >= 15.0);
			assert_eq!(done_stream.result().await.unwrap().duration_ms, measured);

			let failed = AssistantMessageEventStream::new();
			let error = shared(AssistantMessage {
				stop_reason: StopReason::Error,
				..message(pi_host::now_ms(), None)
			});
			failed.push(AssistantMessageEvent::Error {
				reason: StopReason::Error,
				error: error.clone(),
			});
			assert!(error.borrow().duration_ms.unwrap() >= 0.0);

			// TS checks the object passed to end(); Rust's end() takes the message, so the result carries the duration.
			let ended = AssistantMessageEventStream::new();
			let result = message(pi_host::now_ms(), None);
			ended.end(Some(result));
			assert!(ended.result().await.unwrap().duration_ms.unwrap() >= 0.0);
		});
	}

	#[test]
	fn keeps_an_existing_duration_so_a_forwarding_stream_keeps_the_inner_measurement() {
		pi_host::testing::run(|_host| async {
			let outer = AssistantMessageEventStream::new();
			sleep(20.0).await;
			let inner = AssistantMessageEventStream::new();
			let answer = shared(message(pi_host::now_ms(), None));
			inner.push(done(&answer));
			let measured = answer.borrow().duration_ms;
			outer.push(done(&answer));
			assert_eq!(answer.borrow().duration_ms, measured);
			assert!(measured.unwrap() < 20.0);

			let preset = shared(message(pi_host::now_ms(), Some(1234.0)));
			AssistantMessageEventStream::new().push(done(&preset));
			assert_eq!(preset.borrow().duration_ms, Some(1234.0));
		});
	}

	#[test]
	fn leaves_a_message_untimed_when_it_started_before_the_stream_such_as_a_fetched_deferred_result() {
		pi_host::testing::run(|_host| async {
			let fetched = shared(message(pi_host::now_ms() - 60_000.0, None));
			AssistantMessageEventStream::new().push(done(&fetched));
			assert_eq!(fetched.borrow().duration_ms, None);
		});
	}

	#[test]
	fn does_not_time_a_message_pushed_after_the_stream_completed() {
		pi_host::testing::run(|_host| async {
			let stream = AssistantMessageEventStream::new();
			stream.push(done(&shared(message(pi_host::now_ms(), None))));
			let late = shared(message(pi_host::now_ms(), None));
			stream.push(done(&late));
			assert_eq!(late.borrow().duration_ms, None);
		});
	}
}
