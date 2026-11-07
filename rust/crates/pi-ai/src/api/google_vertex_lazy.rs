//! Port of `packages/ai/src/api/google-vertex.lazy.ts`.
//! PORT-STATUS: reviewed

use std::rc::Rc;

use crate::api::google_vertex::GoogleVertexStreams;
use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

/// `googleVertexApi()`: the google-vertex API, loaded on first use.
pub fn google_vertex_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| Box::pin(async { Ok(Rc::new(GoogleVertexStreams) as Rc<dyn ProviderStreams>) })),
		None,
	)
}
