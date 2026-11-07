//! Port of `packages/ai/src/utils/event-stream.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `result()` resolves with a copy of the final message taken when the stream completed (PORTING.md, "Decisions
//!   from wave 1 review"); the TS promise resolves with the live object. `AssistantMessageEventStream::end()` takes
//!   the result by value, so the `durationMs` it sets is not visible on the caller's copy.
//! - `next()` is the async iterator; clones share one queue, like several `for await` loops over one TS stream. A
//!   consumer that drops a pending `next()` gives up its place, so the event goes to the next waiting consumer or the
//!   queue; a TS iterator cannot cancel a pending `next()`.
//! - An `extractResult` that throws in TS (an `AssistantMessageEventStream` result from a non-terminal event) cannot
//!   happen here: `isComplete` admits only `done` and `error`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use futures::channel::oneshot;
use futures::future::Shared;
use futures::{FutureExt, Stream};

use crate::types::{AssistantMessage, AssistantMessageEvent, SharedAssistantMessage};

struct Inner<T, R> {
	queue: VecDeque<T>,
	waiting: VecDeque<oneshot::Sender<Option<T>>>,
	done: bool,
	resolve_final_result: Option<oneshot::Sender<R>>,
	is_complete: Box<dyn Fn(&T) -> bool>,
	extract_result: Box<dyn Fn(&T) -> R>,
}

/// Generic event stream for async iteration. Clones share one queue, like sharing the TS object: each event is
/// delivered to exactly one consumer, in FIFO order of the consumers' `next()` calls.
pub struct EventStream<T, R = T> {
	inner: Rc<RefCell<Inner<T, R>>>,
	final_result: Shared<futures::future::LocalBoxFuture<'static, Option<R>>>,
}

impl<T, R> Clone for EventStream<T, R> {
	fn clone(&self) -> Self {
		Self {
			inner: self.inner.clone(),
			final_result: self.final_result.clone(),
		}
	}
}

impl<T: 'static, R: Clone + 'static> EventStream<T, R> {
	/// `new EventStream(isComplete, extractResult)`.
	pub fn new(is_complete: impl Fn(&T) -> bool + 'static, extract_result: impl Fn(&T) -> R + 'static) -> Self {
		let (resolve, receiver) = oneshot::channel();
		let final_result = receiver.map(Result::ok).boxed_local().shared();
		Self {
			inner: Rc::new(RefCell::new(Inner {
				queue: VecDeque::new(),
				waiting: VecDeque::new(),
				done: false,
				resolve_final_result: Some(resolve),
				is_complete: Box::new(is_complete),
				extract_result: Box::new(extract_result),
			})),
			final_result,
		}
	}

	/// `stream.push(event)`. Ignored after the stream completed.
	pub fn push(&self, event: T) {
		let waiter = {
			let mut inner = self.inner.borrow_mut();
			if inner.done {
				return;
			}
			if (inner.is_complete)(&event) {
				inner.done = true;
				let result = (inner.extract_result)(&event);
				if let Some(resolve) = inner.resolve_final_result.take() {
					let _ = resolve.send(result);
				}
			}
			loop {
				match inner.waiting.pop_front() {
					Some(waiter) if waiter.is_canceled() => continue,
					other => break other,
				}
			}
		};
		match waiter {
			Some(waiter) => {
				if let Err(Some(event)) = waiter.send(Some(event)) {
					self.inner.borrow_mut().queue.push_back(event);
				}
			}
			None => self.inner.borrow_mut().queue.push_back(event),
		}
	}

	/// `stream.end(result)`.
	pub fn end(&self, result: Option<R>) {
		let waiting = {
			let mut inner = self.inner.borrow_mut();
			inner.done = true;
			if let Some(result) = result
				&& let Some(resolve) = inner.resolve_final_result.take()
			{
				let _ = resolve.send(result);
			}
			std::mem::take(&mut inner.waiting)
		};
		for waiter in waiting {
			let _ = waiter.send(None);
		}
	}

	/// One step of `for await (const event of stream)`: the next event, or `None` once the stream is done and
	/// drained.
	pub async fn next(&self) -> Option<T> {
		let receiver = {
			let mut inner = self.inner.borrow_mut();
			if let Some(event) = inner.queue.pop_front() {
				return Some(event);
			}
			if inner.done {
				return None;
			}
			let (sender, receiver) = oneshot::channel();
			inner.waiting.push_back(sender);
			receiver
		};
		receiver.await.ok().flatten()
	}

	/// `await stream.result()`. After `end(None)` it stays pending, like the TS promise. `None` would need the result
	/// sender dropped unsent, which no live handle allows.
	pub async fn result(&self) -> Option<R> {
		self.final_result.clone().await
	}

	/// Whether `push` will still accept events (the TS `protected done` flag, inverted).
	pub fn is_done(&self) -> bool {
		self.inner.borrow().done
	}

	/// Adapts the stream to `futures::Stream` for combinators.
	pub fn into_stream(self) -> impl Stream<Item = T> {
		futures::stream::unfold(self, |stream| async move {
			stream.next().await.map(|event| (event, stream))
		})
	}
}

/// Event stream of one assistant response. It also times the response: the final message (`done` or `error`
/// event, or the result passed to `end()`) gets `durationMs`, measured with a monotonic clock from the stream's
/// creation, unless the message already has one or its `timestamp` predates the stream. A stream that forwards a
/// response which started elsewhere, such as a deferred result fetched later, therefore leaves it untimed.
#[derive(Clone)]
pub struct AssistantMessageEventStream {
	stream: EventStream<AssistantMessageEvent, AssistantMessage>,
	started_at: f64,
	started_at_monotonic: f64,
}

impl AssistantMessageEventStream {
	/// `new AssistantMessageEventStream()`.
	pub fn new() -> Self {
		Self {
			stream: EventStream::new(
				|event: &AssistantMessageEvent| {
					matches!(
						event,
						AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
					)
				},
				|event: &AssistantMessageEvent| match event {
					AssistantMessageEvent::Done { message, .. } => message.borrow().clone(),
					AssistantMessageEvent::Error { error, .. } => error.borrow().clone(),
					_ => panic!("Unexpected event type for final result"),
				},
			),
			started_at: pi_host::now_ms(),
			started_at_monotonic: pi_host::monotonic_ms(),
		}
	}

	/// `stream.push(event)`.
	pub fn push(&self, event: AssistantMessageEvent) {
		match &event {
			AssistantMessageEvent::Done { message, .. } => self.time(message),
			AssistantMessageEvent::Error { error, .. } => self.time(error),
			_ => {}
		}
		self.stream.push(event);
	}

	/// `stream.end(result)`.
	pub fn end(&self, result: Option<AssistantMessage>) {
		let result = result.map(|message| {
			let shared = Rc::new(RefCell::new(message));
			self.time(&shared);
			Rc::try_unwrap(shared)
				.map(RefCell::into_inner)
				.unwrap_or_else(|shared| shared.borrow().clone())
		});
		self.stream.end(result);
	}

	/// See `EventStream::next`.
	pub async fn next(&self) -> Option<AssistantMessageEvent> {
		self.stream.next().await
	}

	/// See `EventStream::result`.
	pub async fn result(&self) -> Option<AssistantMessage> {
		self.stream.result().await
	}

	/// See `EventStream::into_stream`.
	pub fn into_stream(self) -> impl Stream<Item = AssistantMessageEvent> {
		self.stream.into_stream()
	}

	fn time(&self, message: &SharedAssistantMessage) {
		let mut message = message.borrow_mut();
		if self.stream.is_done() || message.duration_ms.is_some() || message.timestamp < self.started_at {
			return;
		}
		message.duration_ms = Some((pi_host::monotonic_ms() - self.started_at_monotonic).round().max(0.0));
	}
}

impl Default for AssistantMessageEventStream {
	fn default() -> Self {
		Self::new()
	}
}

/// Factory function for AssistantMessageEventStream (for use in extensions)
pub fn create_assistant_message_event_stream() -> AssistantMessageEventStream {
	AssistantMessageEventStream::new()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn delivers_in_order_and_resolves_result_once() {
		let stream: EventStream<i32, i32> = EventStream::new(|event| *event < 0, |event| *event);
		let consumer = stream.clone();
		let collected = futures::executor::block_on(async move {
			stream.push(1);
			stream.push(2);
			stream.push(-1);
			stream.push(3);
			let mut seen = Vec::new();
			while let Some(event) = consumer.next().await {
				seen.push(event);
			}
			(seen, consumer.result().await)
		});
		assert_eq!(collected, (vec![1, 2, -1], Some(-1)));
	}

	#[test]
	fn waiting_consumer_is_woken_by_end() {
		let mut pool = futures::executor::LocalPool::new();
		let stream: EventStream<i32, i32> = EventStream::new(|_| false, |event| *event);
		let consumer = stream.clone();
		let handle =
			futures::task::LocalSpawnExt::spawn_local_with_handle(
				&pool.spawner(),
				async move { consumer.next().await },
			)
			.unwrap();
		pool.run_until_stalled();
		stream.end(Some(7));
		assert_eq!(pool.run_until(handle), None);
		assert_eq!(pool.run_until(stream.result()), Some(7));
	}
}
