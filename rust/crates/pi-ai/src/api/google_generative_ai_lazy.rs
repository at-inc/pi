//! Port of `packages/ai/src/api/google-generative-ai.lazy.ts`.
//! PORT-STATUS: ported

use std::rc::Rc;

use crate::api::google_generative_ai::GoogleGenerativeAIStreams;
use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

/// `googleGenerativeAIApi()`: the google-generative-ai API, loaded on first use.
pub fn google_generative_ai_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(GoogleGenerativeAIStreams) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
