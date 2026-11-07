//! Port of `packages/ai/src/api/google-vertex.lazy.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-google-mistral
// Until the API module is ported, the wrapper never panics: every operation fails with "API not ported".
use std::rc::Rc;

use pi_host::PiError;

use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

/// `googleVertexApi()`: every stream ends with an `API not ported` error event.
pub fn google_vertex_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Err(PiError::new("API not ported")) })),
		None,
	)
}
