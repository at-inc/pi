//! Port of `packages/ai/src/providers/typesafe.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use indexmap::IndexMap;

use crate::api::typesafe_system_one_lazy::typesafe_system_one_api;
use crate::auth::helpers::env_api_key_auth;
use crate::auth::types::ProviderAuth;
use crate::models::{CreateProviderOptions, Provider, create_provider};
use crate::providers::typesafe_models::TYPESAFE_CLASSIFIER_MODELS;
use crate::types::AnyModel;

pub fn typesafe_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "typesafe".into(),
		name: Some("TypeSafe".into()),
		auth: ProviderAuth {
			api_key: Some(env_api_key_auth("TypeSafe API key", &["TYPESAFE_API_KEY"])),
			oauth: None,
		},
		models: TYPESAFE_CLASSIFIER_MODELS
			.values()
			.cloned()
			.map(AnyModel::Classifier)
			.collect(),
		classifiers: Some(IndexMap::from([(
			"typesafe-system-one".to_string(),
			typesafe_system_one_api(),
		)])),
		..CreateProviderOptions::default()
	})
	.expect("a provider with classifiers always builds")
}
