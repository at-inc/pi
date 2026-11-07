//! Port of `packages/ai/test/compat-schema.test.ts`.

use pi_ai::providers::compat_schema::{
	ANTHROPIC_MESSAGES_COMPAT_SCHEMA, BEDROCK_COMPAT_SCHEMA, OPENAI_COMPLETIONS_COMPAT_SCHEMA,
	OPENAI_RESPONSES_COMPAT_SCHEMA, PROVIDER_COMPAT_SCHEMA, ProviderCompat, SessionAffinityFormat,
};
use serde_json::{Map, Value, json};

/// The `default` annotation of each property of an object schema.
fn defaults(schema: &Value) -> Map<String, Value> {
	schema["properties"]
		.as_object()
		.expect("object schema")
		.iter()
		.filter_map(|(name, property)| property.get("default").map(|default| (name.clone(), default.clone())))
		.collect()
}

mod compatibility_schemas {
	use super::*;

	#[test]
	fn preserves_property_types_in_the_provider_superset() {
		let compat = ProviderCompat::default();
		let supports_store: Option<bool> = compat.supports_store;
		let session_affinity_format: Option<SessionAffinityFormat> = compat.session_affinity_format;
		assert_eq!(supports_store, None);
		assert_eq!(session_affinity_format, None);
		let parsed: ProviderCompat =
			serde_json::from_value(json!({ "supportsStore": true, "sessionAffinityFormat": "openai-nosession" }))
				.unwrap();
		assert_eq!(parsed.supports_store, Some(true));
		assert_eq!(
			parsed.session_affinity_format,
			Some(SessionAffinityFormat::OpenaiNosession)
		);
	}

	#[test]
	fn preserves_api_specific_defaults() {
		let completions_defaults = defaults(&OPENAI_COMPLETIONS_COMPAT_SCHEMA);
		assert!(!completions_defaults.contains_key("thinkingFormat"));
		assert!(!completions_defaults.contains_key("supportsLongCacheRetention"));
		assert_eq!(
			defaults(&OPENAI_RESPONSES_COMPAT_SCHEMA)["supportsDeveloperRole"],
			json!(true)
		);
		let anthropic_defaults = defaults(&ANTHROPIC_MESSAGES_COMPAT_SCHEMA);
		assert!(!anthropic_defaults.contains_key("sendSessionAffinityHeaders"));
		assert_eq!(anthropic_defaults["supportsLongCacheRetention"], json!(true));
		assert_eq!(defaults(&BEDROCK_COMPAT_SCHEMA)["supportsStrictMode"], json!(false));
	}

	#[test]
	fn does_not_assign_api_specific_defaults_to_the_provider_superset() {
		let provider_defaults = defaults(&PROVIDER_COMPAT_SCHEMA);
		assert!(!provider_defaults.contains_key("supportsDeveloperRole"));
		assert!(!provider_defaults.contains_key("sendSessionAffinityHeaders"));
		assert!(!provider_defaults.contains_key("supportsLongCacheRetention"));
		assert!(!provider_defaults.contains_key("supportsStrictMode"));
	}
}
