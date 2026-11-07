//! Port of `packages/ai/src/api/azure-openai-responses.lazy.ts`.
//! PORT-STATUS: reviewed
//!
//! Rust has no dynamic import: the loader hands out the statically linked module.

use std::rc::Rc;

use crate::api::azure_openai_responses::AzureOpenAIResponsesApi;
use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

pub fn azure_openai_responses_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(AzureOpenAIResponsesApi) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
