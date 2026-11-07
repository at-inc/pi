//! Port of `packages/ai/src/providers/opencode-headers.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - The wrapper is a private struct implementing `ProviderStreams`; the TS `...streams` spread is its forwarding of
//!   the wrapped module's deferred-response methods.

use std::ops::DerefMut;
use std::rc::Rc;

use pi_host::PiResult;

use crate::types::{
	CancelDeferredFunction, FetchDeferredFunction, Model, ProviderHeaders, ProviderStreamOptions, ProviderStreams,
	SimpleStreamOptions, StreamOptions, TranscriptContext,
};
use crate::utils::event_stream::AssistantMessageEventStream;

const OPENCODE_SESSION_HEADER: &str = "x-opencode-session";

fn has_header(headers: Option<&ProviderHeaders>, name: &str) -> bool {
	let expected = name.to_lowercase();
	headers
		.into_iter()
		.flat_map(|headers| headers.keys())
		.any(|key| key.to_lowercase() == expected)
}

fn with_session_header<T: DerefMut<Target = StreamOptions>>(options: Option<T>) -> Option<T> {
	let mut options = options?;
	let Some(session_id) = options.session_id.clone().filter(|session_id| !session_id.is_empty()) else {
		return Some(options);
	};
	if has_header(options.headers.as_ref(), OPENCODE_SESSION_HEADER) {
		return Some(options);
	}
	options
		.headers
		.get_or_insert_with(ProviderHeaders::new)
		.insert(OPENCODE_SESSION_HEADER.to_string(), Some(session_id));
	Some(options)
}

struct OpenCodeSessionHeaderStreams {
	streams: Rc<dyn ProviderStreams>,
}

impl ProviderStreams for OpenCodeSessionHeaderStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		self.streams.stream(model, context, with_session_header(options))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		self.streams.stream_simple(model, context, with_session_header(options))
	}

	fn fetch_deferred(&self) -> Option<FetchDeferredFunction> {
		self.streams.fetch_deferred()
	}

	fn cancel_deferred(&self) -> Option<CancelDeferredFunction> {
		self.streams.cancel_deferred()
	}
}

/// Adds OpenCode's required per-conversation routing header before API dispatch.
pub fn with_open_code_session_header(streams: Rc<dyn ProviderStreams>) -> Rc<dyn ProviderStreams> {
	Rc::new(OpenCodeSessionHeaderStreams { streams })
}
