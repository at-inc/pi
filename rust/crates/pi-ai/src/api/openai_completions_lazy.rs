//! Port of `packages/ai/src/api/openai-completions.lazy.ts`.
//! PORT-STATUS: ported

use std::rc::Rc;

use crate::api::lazy::lazy_api;
use crate::api::openai_completions::OpenAICompletionsStreams;
use crate::types::ProviderStreams;

/// `openAICompletionsApi()`: the openai-completions API, loaded on first use.
pub fn openai_completions_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(OpenAICompletionsStreams) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
