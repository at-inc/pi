//! Port of `packages/ai/src/providers/faux.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Only the items `compat/mod.rs` uses, with the signatures of w2-ai-core's port on `capy/rust-port-w2-ai-core` (c990cc571), which replaces this stub.
use std::rc::Rc;

use crate::types::{Model, SimpleStreamOptions, TranscriptContext};
use crate::utils::event_stream::AssistantMessageEventStream;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegisterFauxProviderOptions {}

pub struct FauxProviderRegistration {
	pub core: FauxCore,
	pub unregister: Rc<dyn Fn()>,
}

/// What `createFauxCore()` returns. Clones share one core.
#[derive(Clone)]
pub struct FauxCore {
	pub api: String,
}

impl FauxCore {
	/// `core.stream(model, context, options)`.
	pub fn stream(
		&self,
		_request_model: &Model,
		_context: &TranscriptContext,
		_stream_options: Option<SimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		todo!("PORT-STUB")
	}

	/// `core.streamSimple(model, context, options)`.
	pub fn stream_simple(
		&self,
		_stream_model: &Model,
		_context: &TranscriptContext,
		_stream_options: Option<SimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		todo!("PORT-STUB")
	}
}

pub fn create_faux_core(_options: RegisterFauxProviderOptions) -> FauxCore {
	todo!("PORT-STUB")
}
