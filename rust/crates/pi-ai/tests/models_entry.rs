//! Port of `packages/ai/test/models-entry.test.ts`.
//!
//! The TS test spawns Node with a module-load hook that fails on TypeBox, provider SDKs, catalogs, or generated model
//! data, once for the source entry and once for the `@at-inc/pi-ai/models` export. Rust has no lazy module graph and one
//! crate entry, so the hook and the `it.each` over two entries are not ported; the faux completion the child script
//! runs through `createModels` is. `typeof createProvider === "function"` holds by type.

use pi_ai::models::create_models;
use pi_ai::providers::faux::{faux_assistant_message, faux_provider, faux_text};
use pi_ai::types::{AssistantContent, Context, StopReason};

mod lightweight_models_entry {
	use super::*;

	#[test]
	fn runs_a_faux_completion_without_typebox_catalogs_or_sdks() {
		pi_host::testing::run(|_| async {
			let models = create_models(None);
			assert_eq!(models.get_models(None), Vec::new());
			assert!(models.get_providers().is_empty());
			let faux = faux_provider(None);
			models.set_provider(faux.provider.clone());
			faux.set_responses(vec![faux_assistant_message("OK", None).into()]);
			let response = models
				.complete_simple(&faux.get_model(None).unwrap(), &Context::default(), None)
				.await
				.unwrap();
			assert_eq!(response.stop_reason, StopReason::Stop);
			assert_eq!(response.content, vec![AssistantContent::Text(faux_text("OK"))]);
		});
	}
}
