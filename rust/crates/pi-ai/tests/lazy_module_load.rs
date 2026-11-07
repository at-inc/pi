//! Port of `packages/ai/test/lazy-module-load.test.ts`.
//!
//! The TS suite spawns Node with a resolve hook and checks which provider SDK packages get imported: none for the root
//! barrel, `builtinModels()`, or the compat entry, and only `@anthropic-ai/sdk` for an Anthropic stream through the
//! lazy wrapper or `streamSimple`. Rust links every module up front, so those five module-graph checks are not ported.
//! What carries over is `lazyApi`'s contract that they rely on: the implementation module loads on the first stream
//! call, not when the wrapper is built, and a load failure ends the returned stream with an error event. The tests
//! below check that contract with a counting loader standing in for the dynamic import. Rust has no import cache, so
//! the loader runs on every stream call and decides itself whether to reuse a module (see `api/lazy.rs`).

use std::cell::Cell;
use std::rc::Rc;

use pi_ai::api::lazy::{LazyApiLoader, lazy_api};
use pi_ai::types::{
	AssistantMessage, AssistantMessageEvent, BaseModel, Context, Message, Model, ModelInputModality,
	ProviderStreamOptions, ProviderStreams, SimpleStreamOptions, StopReason, TranscriptContext, UserMessage,
	UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::{PiError, PiResult};

fn anthropic_model() -> Model {
	Model {
		base: BaseModel {
			id: "claude-sonnet-4-6".into(),
			name: "Claude Sonnet 4".into(),
			api: "anthropic-messages".into(),
			provider: "anthropic".into(),
			base_url: "https://api.anthropic.com".into(),
			input: vec![ModelInputModality::Text],
			..BaseModel::default()
		},
		reasoning: true,
		context_window: 200000.0,
		max_tokens: 8192.0,
		..Model::default()
	}
}

fn context() -> TranscriptContext {
	normalize_context(&Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("hi".into()),
			timestamp: pi_host::now_ms(),
			..UserMessage::default()
		})],
		..Context::default()
	})
}

/// The loaded module: answers every request with one `done` message.
struct LoadedApi;

impl LoadedApi {
	fn respond(model: &Model) -> AssistantMessageEventStream {
		let stream = AssistantMessageEventStream::new();
		let message = AssistantMessage {
			api: model.api.clone(),
			provider: model.provider.clone(),
			model: model.id.clone(),
			stop_reason: StopReason::Stop,
			timestamp: pi_host::now_ms(),
			..AssistantMessage::default()
		};
		stream.push(AssistantMessageEvent::Done {
			reason: StopReason::Stop,
			message: Rc::new(std::cell::RefCell::new(message.clone())),
		});
		stream.end(Some(message));
		stream
	}
}

impl ProviderStreams for LoadedApi {
	fn stream(
		&self,
		model: &Model,
		_context: &TranscriptContext,
		_options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(Self::respond(model))
	}

	fn stream_simple(
		&self,
		model: &Model,
		_context: &TranscriptContext,
		_options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(Self::respond(model))
	}
}

/// A loader standing in for `import("./anthropic-messages.ts")` that counts its calls.
fn counting_loader(loads: &Rc<Cell<u32>>, result: Result<(), &'static str>) -> LazyApiLoader {
	let loads = loads.clone();
	Rc::new(move || {
		loads.set(loads.get() + 1);
		Box::pin(async move {
			match result {
				Ok(()) => Ok(Rc::new(LoadedApi) as Rc<dyn ProviderStreams>),
				Err(message) => Err(PiError::new(message)),
			}
		})
	})
}

mod lazy_provider_module_loading {
	use super::*;

	#[test]
	fn does_not_load_the_api_module_when_building_the_lazy_wrapper() {
		let loads = Rc::new(Cell::new(0));
		let _api = lazy_api(counting_loader(&loads, Ok(())), None);
		assert_eq!(loads.get(), 0);
	}

	#[test]
	fn loads_the_api_module_when_streaming_through_the_lazy_api_wrapper() {
		pi_host::testing::run(|_| async {
			let loads = Rc::new(Cell::new(0));
			let api = lazy_api(counting_loader(&loads, Ok(())), None);

			let stream = api.stream_simple(&anthropic_model(), &context(), None).unwrap();
			assert_eq!(loads.get(), 1);
			let result = stream.result().await.unwrap();
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(result.model, "claude-sonnet-4-6");

			api.stream(&anthropic_model(), &context(), None)
				.unwrap()
				.result()
				.await
				.unwrap();
			assert_eq!(loads.get(), 2);
		});
	}

	#[test]
	fn ends_the_stream_with_an_error_event_when_the_api_module_fails_to_load() {
		pi_host::testing::run(|_| async {
			let loads = Rc::new(Cell::new(0));
			let api = lazy_api(
				counting_loader(&loads, Err("Cannot find package '@anthropic-ai/sdk'")),
				None,
			);

			let stream = api.stream_simple(&anthropic_model(), &context(), None).unwrap();
			let mut events = Vec::new();
			while let Some(event) = stream.next().await {
				events.push(event);
			}
			assert!(matches!(
				events.as_slice(),
				[AssistantMessageEvent::Error {
					reason: StopReason::Error,
					..
				}]
			));
			let result = stream.result().await.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Cannot find package '@anthropic-ai/sdk'")
			);
			assert_eq!(loads.get(), 1);
		});
	}
}
