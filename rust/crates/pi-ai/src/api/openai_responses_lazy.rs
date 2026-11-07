//! Port of `packages/ai/src/api/openai-responses.lazy.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-openai-responses
// Until the API module is ported, the wrapper never panics: every operation fails with "API not ported".
use std::rc::Rc;

use pi_host::PiError;

use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

/// `openAIResponsesApi()`: every stream ends with an `API not ported` error event.
pub fn openai_responses_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Err(PiError::new("API not ported")) })),
		None,
	)
}
