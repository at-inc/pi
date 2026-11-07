//! Port of `packages/ai/src/api/lazy.ts`.
//! PORT-STATUS: reviewed
//!
//! `lazyStream`'s `setup` is a closure returning a future: it is called synchronously, like the TS `setup()` call,
//! so the part of a TS async setup body before its first `await` belongs in the closure, and the future it returns runs
//! on the host executor. Its `AsyncIterable` source is a [`LazySource`]: an `AssistantMessageEventStream`, which has
//! the `result()` the TS `hasResult` check looks for, or a plain event stream, which has none. Observers and sources
//! cannot fail in Rust, so the TS `.catch` covers only setup failures. `onResult` receives the source's result, not
//! the outer stream's copy of it. `lazyApi`'s `load` returns the implementation module as `Rc<dyn ProviderStreams>`;
//! Rust has no import cache, so `load` itself decides whether to reuse a module. `lazyApi` passes the implementation
//! clones of `model` and `context` taken at call time.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::oneshot;
use futures::stream::LocalBoxStream;

use pi_host::{LocalBoxFuture, PiError, PiResult};

use crate::types::{
	AssistantMessage, AssistantMessageEvent, CancelDeferredFunction, DeferredCancelOptions, DeferredFetchOptions,
	DeferredHandle, FetchDeferredFunction, Model, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions,
	StopReason, TranscriptContext, Usage, UsageCost,
};
use crate::utils::event_stream::AssistantMessageEventStream;

fn create_setup_error_message(model: &Model, error: &PiError, timestamp: f64) -> AssistantMessage {
	AssistantMessage {
		content: Vec::new(),
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage {
			input: 0.0,
			output: 0.0,
			cache_read: 0.0,
			cache_write: 0.0,
			total_tokens: 0.0,
			cost: UsageCost {
				input: 0.0,
				output: 0.0,
				cache_read: 0.0,
				cache_write: 0.0,
				total: 0.0,
				..UsageCost::default()
			},
			..Usage::default()
		},
		stop_reason: StopReason::Error,
		error_message: Some(error.message.clone()),
		timestamp,
		..AssistantMessage::default()
	}
}

/// `LazyStreamObserver.onEvent`.
pub type LazyStreamEventObserver = Rc<dyn Fn(&AssistantMessageEvent)>;
/// `LazyStreamObserver.onResult`.
pub type LazyStreamResultObserver = Rc<dyn Fn(Option<&AssistantMessage>)>;

/// Synchronous, read-only observation of the stream's existing forwarding path.
#[derive(Clone, Default)]
pub struct LazyStreamObserver {
	pub on_event: Option<LazyStreamEventObserver>,
	pub on_result: Option<LazyStreamResultObserver>,
}

impl LazyStreamObserver {
	fn event(&self, event: &AssistantMessageEvent) {
		if let Some(on_event) = &self.on_event {
			on_event(event);
		}
	}
}

/// The `AsyncIterable<AssistantMessageEvent>` a `lazyStream` setup resolves with.
pub enum LazySource {
	/// An event stream with a `result()`.
	Stream(AssistantMessageEventStream),
	/// A plain iterable without `result()`: the last `done` or `error` message is the result.
	Events(LocalBoxStream<'static, AssistantMessageEvent>),
}

impl From<AssistantMessageEventStream> for LazySource {
	fn from(stream: AssistantMessageEventStream) -> Self {
		LazySource::Stream(stream)
	}
}

async fn forward_stream(
	target: &AssistantMessageEventStream,
	source: LazySource,
	observer: Option<&LazyStreamObserver>,
	observe_result: &dyn Fn(Option<&AssistantMessage>),
) {
	let (stream, mut events) = match source {
		LazySource::Stream(stream) => (Some(stream), None),
		LazySource::Events(events) => (None, Some(events)),
	};
	let mut result: Option<AssistantMessage> = None;
	loop {
		let event = match (&stream, &mut events) {
			(Some(stream), _) => stream.next().await,
			(None, Some(events)) => events.next().await,
			(None, None) => None,
		};
		let Some(event) = event else {
			break;
		};
		target.push(event.clone());
		if let Some(observer) = observer {
			observer.event(&event);
		}
		match &event {
			AssistantMessageEvent::Done { message, .. } => result = Some(message.borrow().clone()),
			AssistantMessageEvent::Error { error, .. } => result = Some(error.borrow().clone()),
			_ => {}
		}
	}
	if let Some(stream) = &stream {
		result = stream.result().await.or(result);
	}
	// TS `for await` and `await source.result()` take a microtask before the result is observed, so whoever awaits the
	// outer stream's result (resolved by the pushed `done`) runs first.
	yield_now().await;
	target.end(result.clone());
	observe_result(result.as_ref());
}

/// Lets every task that is already woken run before this one continues.
async fn yield_now() {
	let mut yielded = false;
	futures::future::poll_fn(|context| {
		if yielded {
			return std::task::Poll::Ready(());
		}
		yielded = true;
		context.waker().wake_by_ref();
		std::task::Poll::Pending
	})
	.await;
}

/// Runs `operation` on the host executor from now on, like the body of a TS async function, which runs whether or
/// not anyone awaits its promise. The returned future only waits for the outcome, so dropping it cancels nothing.
pub(crate) fn start_now<T: 'static>(operation: impl Future<Output = T> + 'static) -> LocalBoxFuture<'static, T> {
	let (sender, receiver) = oneshot::channel();
	pi_host::spawn_local(async move {
		let _ = sender.send(operation.await);
	});
	Box::pin(async move {
		match receiver.await {
			Ok(outcome) => outcome,
			Err(_) => futures::future::pending().await,
		}
	})
}

/// Returns a stream synchronously while running async setup (auth resolution,
/// lazy module loading) behind it. Setup failures terminate the stream with an
/// error event.
pub fn lazy_stream<F, Fut, S>(
	model: &Model,
	setup: F,
	observer: Option<LazyStreamObserver>,
) -> AssistantMessageEventStream
where
	F: FnOnce() -> Fut,
	Fut: Future<Output = PiResult<S>> + 'static,
	S: Into<LazySource>,
{
	let started_at = pi_host::now_ms();
	let outer = AssistantMessageEventStream::new();
	let observed_result = Rc::new(Cell::new(false));
	let observe_result = {
		let observer = observer.clone();
		move |result: Option<&AssistantMessage>| {
			if observed_result.get() {
				return;
			}
			observed_result.set(true);
			if let Some(on_result) = observer.as_ref().and_then(|observer| observer.on_result.as_ref()) {
				on_result(result);
			}
		}
	};

	let setup = setup();
	let target = outer.clone();
	let model = model.clone();
	pi_host::spawn_local(async move {
		match setup.await {
			Ok(inner) => forward_stream(&target, inner.into(), observer.as_ref(), &observe_result).await,
			Err(error) => {
				let message = Rc::new(std::cell::RefCell::new(create_setup_error_message(
					&model, &error, started_at,
				)));
				let event = AssistantMessageEvent::Error {
					reason: StopReason::Error,
					error: message.clone(),
				};
				target.push(event.clone());
				if let Some(observer) = &observer {
					observer.event(&event);
				}
				let message = message.borrow().clone();
				target.end(Some(message.clone()));
				observe_result(Some(&message));
			}
		}
	});

	outer
}

/// Which optional deferred-response methods a lazily loaded API module provides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LazyApiCapabilities {
	pub fetch_deferred: Option<bool>,
	pub cancel_deferred: Option<bool>,
}

/// Loads an API implementation module.
pub type LazyApiLoader = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<Rc<dyn ProviderStreams>>>>;

/// `lazyApi(...)`'s `ProviderStreams`.
struct LazyApi {
	load: LazyApiLoader,
	capabilities: LazyApiCapabilities,
}

impl ProviderStreams for LazyApi {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let load = self.load.clone();
		let (request_model, context) = (model.clone(), context.clone());
		Ok(lazy_stream(
			model,
			move || {
				let loading = load();
				async move { loading.await?.stream(&request_model, &context, options) }
			},
			None,
		))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let load = self.load.clone();
		let (request_model, context) = (model.clone(), context.clone());
		Ok(lazy_stream(
			model,
			move || {
				let loading = load();
				async move { loading.await?.stream_simple(&request_model, &context, options) }
			},
			None,
		))
	}

	fn fetch_deferred(&self) -> Option<FetchDeferredFunction> {
		if self.capabilities.fetch_deferred != Some(true) {
			return None;
		}
		let load = self.load.clone();
		Some(Rc::new(
			move |model: &Model, handle: &DeferredHandle, options: Option<DeferredFetchOptions>| {
				let load = load.clone();
				let (request_model, handle) = (model.clone(), handle.clone());
				Ok(lazy_stream(
					model,
					move || {
						let loading = load();
						async move {
							let implementation = loading.await?;
							let Some(fetch_deferred) = implementation.fetch_deferred() else {
								return Err(PiError::new("API does not support deferred responses"));
							};
							fetch_deferred(&request_model, &handle, options)
						}
					},
					None,
				))
			},
		))
	}

	fn cancel_deferred(&self) -> Option<CancelDeferredFunction> {
		if self.capabilities.cancel_deferred != Some(true) {
			return None;
		}
		let load = self.load.clone();
		Some(Rc::new(
			move |model: &Model, handle: &DeferredHandle, options: Option<DeferredCancelOptions>| {
				let loading = load();
				let (model, handle) = (model.clone(), handle.clone());
				start_now(async move {
					let implementation = loading.await?;
					let Some(cancel_deferred) = implementation.cancel_deferred() else {
						return Err(PiError::new("API cannot cancel deferred responses"));
					};
					cancel_deferred(&model, &handle, options).await
				})
			},
		))
	}
}

/// Wraps a dynamically imported API implementation module as `ProviderStreams`.
/// The module loads on first stream call. Load failures terminate the returned stream with an error event.
pub fn lazy_api(load: LazyApiLoader, capabilities: Option<LazyApiCapabilities>) -> Rc<dyn ProviderStreams> {
	Rc::new(LazyApi {
		load,
		capabilities: capabilities.unwrap_or_default(),
	})
}
