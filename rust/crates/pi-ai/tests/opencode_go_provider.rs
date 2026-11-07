//! Port of `packages/ai/test/opencode-go-provider.test.ts`.
//!
//! Not ported: the dispatch half of `routes %s through Google Generative AI with the OpenCode session header`.
//! It replaces `api/google-generative-ai.lazy.ts` with `vi.mock`; Rust has no module mocking, and the real
//! Google API is wave-3 (`w3-google-mistral`). The session-header wrapper it relies on is covered by
//! `opencode_provider_headers.rs`. The model lookup half is ported.

use pi_ai::providers::opencode_go::opencode_go_provider;

mod open_code_go_gemini_dispatch {
	use super::*;

	#[test]
	fn routes_gemini_models_through_google_generative_ai() {
		let provider = opencode_go_provider();
		let model = (provider.get_models)()
			.into_iter()
			.find(|model| model.id == "gemini-4-argon")
			.expect("OpenCode Go Gemini model is missing");
		assert_eq!(model.api, "google-generative-ai");
		assert_eq!(model.provider, "opencode-go");
		assert_eq!(model.base_url, "https://opencode.ai/inference/go/google/v1beta");
	}
}
