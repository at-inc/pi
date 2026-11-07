//! Port of `packages/ai/test/opencode-provider-headers.test.ts`.
//!
//! `fauxAssistantMessage("ok")` (providers/faux.ts, pending) is built inline as a text-only assistant message.
//! `it.each` cases run as one test per case.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::providers::opencode_headers::with_open_code_session_header;
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, BaseModel, CacheRetention, Context, Message, Model,
	ModelCost, ModelInputModality, ProviderHeaders, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions,
	StopReason, StreamOptions, TextContent, TranscriptContext, UserMessage, UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiResult;

fn model() -> Model {
	Model {
		base: BaseModel {
			id: "test-model".into(),
			name: "Test model".into(),
			api: "test-api".into(),
			provider: "opencode".into(),
			base_url: "https://opencode.ai/zen/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 1000.0,
		max_tokens: 100.0,
		..Model::default()
	}
}

fn context() -> TranscriptContext {
	normalize_context(&Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("hi".into()),
			timestamp: 0.0,
			..UserMessage::default()
		})],
		..Context::default()
	})
}

fn fake_assistant_message(text: &str) -> AssistantMessage {
	AssistantMessage {
		content: vec![AssistantContent::Text(TextContent {
			text: text.into(),
			..TextContent::default()
		})],
		api: "faux".into(),
		provider: "faux".into(),
		model: "faux-1".into(),
		stop_reason: StopReason::Stop,
		..AssistantMessage::default()
	}
}

fn completed_stream() -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let message = Rc::new(RefCell::new(fake_assistant_message("ok")));
	stream.push(AssistantMessageEvent::Start {
		partial: message.clone(),
	});
	stream.push(AssistantMessageEvent::Done {
		reason: StopReason::Stop,
		message: message.clone(),
	});
	stream.end(Some(message.borrow().clone()));
	stream
}

/// The headers of the options each call received: `None` before any call, `Some(None)` for no headers.
type Captured = Rc<RefCell<Option<Option<ProviderHeaders>>>>;

struct RecordingStreams {
	captured: Captured,
}

impl ProviderStreams for RecordingStreams {
	fn stream(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		*self.captured.borrow_mut() = Some(options.and_then(|options| options.headers.clone()));
		Ok(completed_stream())
	}

	fn stream_simple(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		*self.captured.borrow_mut() = Some(options.and_then(|options| options.headers.clone()));
		Ok(completed_stream())
	}
}

fn recording_streams() -> (Rc<dyn ProviderStreams>, Captured) {
	let captured: Captured = Rc::new(RefCell::new(None));
	let streams = with_open_code_session_header(Rc::new(RecordingStreams {
		captured: captured.clone(),
	}));
	(streams, captured)
}

fn headers(entries: &[(&str, Option<&str>)]) -> ProviderHeaders {
	entries
		.iter()
		.map(|(name, value)| (name.to_string(), value.map(str::to_string)))
		.collect::<IndexMap<_, _>>()
}

fn stream_options(base: StreamOptions) -> ProviderStreamOptions {
	ProviderStreamOptions {
		base,
		..ProviderStreamOptions::default()
	}
}

fn simple_options(base: StreamOptions) -> SimpleStreamOptions {
	SimpleStreamOptions {
		base,
		..SimpleStreamOptions::default()
	}
}

mod open_code_provider_headers {
	use super::*;

	fn session_without_cache_retention() -> StreamOptions {
		StreamOptions {
			session_id: Some("conversation-1".into()),
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		}
	}

	// Regression test for https://github.com/earendil-works/pi/issues/9326
	#[test]
	fn maps_session_id_for_stream_requests_even_without_cache_retention() {
		pi_host::testing::run(|_| async {
			let (streams, captured) = recording_streams();
			streams
				.stream(
					&model(),
					&context(),
					Some(stream_options(session_without_cache_retention())),
				)
				.unwrap();
			assert_eq!(
				*captured.borrow(),
				Some(Some(headers(&[("x-opencode-session", Some("conversation-1"))])))
			);
		});
	}

	// Regression test for https://github.com/earendil-works/pi/issues/9326
	#[test]
	fn maps_session_id_for_stream_simple_requests_even_without_cache_retention() {
		pi_host::testing::run(|_| async {
			let (streams, captured) = recording_streams();
			streams
				.stream_simple(
					&model(),
					&context(),
					Some(simple_options(session_without_cache_retention())),
				)
				.unwrap();
			assert_eq!(
				*captured.borrow(),
				Some(Some(headers(&[("x-opencode-session", Some("conversation-1"))])))
			);
		});
	}

	#[test]
	fn preserves_a_case_insensitive_caller_override() {
		pi_host::testing::run(|_| async {
			for value in [Some("caller-value"), None] {
				let (streams, captured) = recording_streams();
				let caller_headers = headers(&[("X-OpenCode-Session", value)]);
				streams
					.stream_simple(
						&model(),
						&context(),
						Some(simple_options(StreamOptions {
							session_id: Some("generated-value".into()),
							base: pi_ai::types::ProviderRequestOptions {
								headers: Some(caller_headers.clone()),
								..Default::default()
							},
							..StreamOptions::default()
						})),
					)
					.unwrap();
				assert_eq!(*captured.borrow(), Some(Some(caller_headers)));
			}
		});
	}

	#[test]
	fn does_not_fabricate_a_session_header_when_session_id_is_absent() {
		pi_host::testing::run(|_| async {
			let (streams, captured) = recording_streams();
			streams
				.stream_simple(
					&model(),
					&context(),
					Some(simple_options(StreamOptions {
						base: pi_ai::types::ProviderRequestOptions {
							headers: Some(headers(&[("x-custom", Some("value"))])),
							..Default::default()
						},
						..StreamOptions::default()
					})),
				)
				.unwrap();
			assert_eq!(*captured.borrow(), Some(Some(headers(&[("x-custom", Some("value"))]))));
		});
	}
}
