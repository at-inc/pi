//! Port of `packages/ai/src/api/openai-codex-responses.lazy.ts`.
//! PORT-STATUS: reviewed
//!
//! Rust has no dynamic import: the loader hands out the statically linked module.

use std::rc::Rc;

use crate::api::lazy::lazy_api;
use crate::api::openai_codex_responses::OpenAICodexResponsesApi;
use crate::types::ProviderStreams;

pub fn openai_codex_responses_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(OpenAICodexResponsesApi) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
