//! Port of `packages/ai/src/api/anthropic-messages.lazy.ts`.
//! PORT-STATUS: ported
//!
//! Rust has no dynamic import: `load` hands out the module's `ProviderStreams` value.

use std::rc::Rc;

use crate::api::anthropic_messages::AnthropicMessagesStreams;
use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

/// `anthropicMessagesApi()`.
pub fn anthropic_messages_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(AnthropicMessagesStreams) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
