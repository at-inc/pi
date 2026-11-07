//! Port of `packages/ai/src/api/mistral-conversations.lazy.ts`.
//! PORT-STATUS: ported

use std::rc::Rc;

use crate::api::lazy::lazy_api;
use crate::api::mistral_conversations::MistralConversationsStreams;
use crate::types::ProviderStreams;

/// `mistralConversationsApi()`: the mistral-conversations API, loaded on first use.
pub fn mistral_conversations_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(MistralConversationsStreams) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
