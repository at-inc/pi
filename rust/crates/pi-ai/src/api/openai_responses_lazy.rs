//! Port of `packages/ai/src/api/openai-responses.lazy.ts`.
//! PORT-STATUS: reviewed
//!
//! Rust has no dynamic import: the loader hands out the statically linked module.

use std::rc::Rc;

use crate::api::lazy::lazy_api;
use crate::api::openai_responses::OpenAIResponsesApi;
use crate::types::ProviderStreams;

pub fn openai_responses_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(OpenAIResponsesApi) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
