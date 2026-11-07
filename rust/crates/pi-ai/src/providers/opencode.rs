//! Port of `packages/ai/src/providers/opencode.ts`.
//! PORT-STATUS: reviewed
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use indexmap::IndexMap;

use crate::api::anthropic_messages_lazy::anthropic_messages_api;
use crate::api::google_generative_ai_lazy::google_generative_ai_api;
use crate::api::openai_completions_lazy::openai_completions_api;
use crate::api::openai_responses_lazy::openai_responses_api;
use crate::api::typesafe_system_one_lazy::typesafe_system_one_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::opencode_headers::with_open_code_session_header;
use crate::providers::opencode_models::{OPENCODE_CLASSIFIER_MODELS, OPENCODE_MODELS};
use crate::types::AnyModel;

pub fn opencode_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "opencode".into(),
		name: Some("OpenCode Zen".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("OpenCode API key", &["OPENCODE_API_KEY"])),
			oauth: None,
		},
		models: OPENCODE_MODELS
			.values()
			.cloned()
			.map(AnyModel::Chat)
			.chain(OPENCODE_CLASSIFIER_MODELS.values().cloned().map(AnyModel::Classifier))
			.collect(),
		api: Some(ProviderApi::ByApi(IndexMap::from([
			(
				"anthropic-messages".to_string(),
				with_open_code_session_header(anthropic_messages_api()),
			),
			(
				"google-generative-ai".to_string(),
				with_open_code_session_header(google_generative_ai_api()),
			),
			(
				"openai-completions".to_string(),
				with_open_code_session_header(openai_completions_api()),
			),
			(
				"openai-responses".to_string(),
				with_open_code_session_header(openai_responses_api()),
			),
		]))),
		// OpenCode Zen serves TypeSafe's System One protocol at /inference/systemone/v1/systemone.
		classifiers: Some(IndexMap::from([(
			"typesafe-system-one".to_string(),
			typesafe_system_one_api(),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
