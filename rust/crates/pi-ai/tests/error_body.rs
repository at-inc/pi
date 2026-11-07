//! Port of `packages/ai/test/error-body.test.ts`.
//!
//! The SDK error objects become `PiError`s with the SDK fields in `data`. The TS cases built on class instances and
//! streams, and the non-`Error` thrown value, have no Rust counterpart: `PiError.data` is plain JSON. The heap test of
//! `truncateErrorText` checks V8 string slices and becomes a plain truncation check.

use pi_ai::utils::error_body::{
	MAX_PROVIDER_ERROR_BODY_CHARS, format_provider_error, normalize_provider_error, truncate_error_text,
};
use pi_host::PiError;
use serde_json::{Value, json};

fn sdk_error(message: &str, data: Value) -> PiError {
	PiError::new(message).with_data(data)
}

mod normalize_provider_error {
	use super::*;

	#[test]
	fn extracts_status_and_body_from_a_mistral_shaped_error() {
		let error = sdk_error(
			"Mistral request failed",
			json!({ "statusCode": 403, "body": "{\"error\":\"blocked by gateway WAF\"}" }),
		);
		let norm = normalize_provider_error(&error);
		assert_eq!(norm.status, Some(403.0));
		assert_eq!(norm.body.as_deref(), Some("{\"error\":\"blocked by gateway WAF\"}"));
		assert!(!norm.message_carries_body);
	}

	#[test]
	fn reads_the_parsed_body_off_an_openai_api_error_when_the_message_is_opaque() {
		let error = sdk_error(
			"403 status code (no body)",
			json!({ "error": { "error": "blocked by gateway WAF" } }),
		)
		.with_status(403);
		let norm = normalize_provider_error(&error);
		assert_eq!(norm.status, Some(403.0));
		assert_eq!(norm.body.as_deref(), Some("{\"error\":\"blocked by gateway WAF\"}"));
		assert!(!norm.message_carries_body);
	}

	#[test]
	fn preserves_the_message_when_google_genai_already_folds_the_body_into_it() {
		let body = "{\"error\":{\"code\":403,\"message\":\"Permission denied\"}}";
		let norm = normalize_provider_error(&PiError::new(body).with_status(403));
		assert_eq!(norm.status, Some(403.0));
		assert!(norm.message_carries_body);
		assert_eq!(norm.message, body);
	}

	#[test]
	fn extracts_status_and_body_from_a_bedrock_shaped_service_exception() {
		let error = PiError::named("UnknownError", "UnknownError").with_data(json!({
			"$metadata": { "httpStatusCode": 403 },
			"$response": { "statusCode": 403, "body": "{\"message\":\"blocked by gateway WAF\"}" },
		}));
		let norm = normalize_provider_error(&error);
		assert_eq!(norm.status, Some(403.0));
		assert_eq!(norm.body.as_deref(), Some("{\"message\":\"blocked by gateway WAF\"}"));
		assert!(!norm.message_carries_body);
	}

	#[test]
	fn still_surfaces_a_plain_parsed_json_body_object() {
		let error = sdk_error(
			"400 status code (no body)",
			json!({ "error": { "message": "schema validation failed", "field": "tools[0]" } }),
		)
		.with_status(400);
		let norm = normalize_provider_error(&error);
		assert_eq!(
			norm.body.as_deref(),
			Some("{\"message\":\"schema validation failed\",\"field\":\"tools[0]\"}")
		);
		assert!(!norm.message_carries_body);
	}

	#[test]
	fn treats_an_empty_parsed_body_object_as_no_body() {
		let error = sdk_error("403 status code (no body)", json!({ "error": {} })).with_status(403);
		let norm = normalize_provider_error(&error);
		assert_eq!(norm.body, None);
		assert!(norm.message_carries_body);
	}

	#[test]
	fn truncates_the_body_at_the_cap() {
		let long_body = "x".repeat(MAX_PROVIDER_ERROR_BODY_CHARS + 50);
		let error = sdk_error("failed", json!({ "statusCode": 500, "body": long_body }));
		let body = normalize_provider_error(&error).body.unwrap();
		assert!(body.contains("... [truncated 50 chars]"));
		assert!(body.len() < long_body.len());
	}

	#[test]
	fn sets_message_carries_body_when_the_message_already_contains_the_extracted_body() {
		let error = sdk_error(
			"500: upstream exploded",
			json!({ "statusCode": 500, "body": "upstream exploded" }),
		);
		assert!(normalize_provider_error(&error).message_carries_body);
	}
}

mod format_provider_error {
	use super::*;

	fn opaque_openai_error() -> PiError {
		sdk_error(
			"403 status code (no body)",
			json!({ "error": { "error": "blocked by gateway WAF" } }),
		)
		.with_status(403)
	}

	#[test]
	fn surfaces_status_and_body_without_a_prefix() {
		let formatted = format_provider_error(&normalize_provider_error(&opaque_openai_error()), None);
		assert!(formatted.contains("403"));
		assert!(formatted.contains("blocked by gateway WAF"));
		assert_ne!(formatted, "403 status code (no body)");
	}

	#[test]
	fn applies_a_provider_prefix_with_status_and_body() {
		assert_eq!(
			format_provider_error(
				&normalize_provider_error(&opaque_openai_error()),
				Some("OpenAI API error")
			),
			"OpenAI API error (403): {\"error\":\"blocked by gateway WAF\"}"
		);
	}

	#[test]
	fn preserves_the_message_with_prefix_and_status_when_it_already_carries_the_body() {
		let body = "{\"error\":{\"message\":\"Permission denied\"}}";
		let norm = normalize_provider_error(&PiError::new(body).with_status(403));
		assert_eq!(
			format_provider_error(&norm, Some("OpenAI API error")),
			format!("OpenAI API error (403): {body}")
		);
	}
}

// #9265
mod truncate_error_text {
	use super::*;

	#[test]
	fn keeps_an_excerpt_of_the_body() {
		let body = "<html>".repeat(1_000_000);
		let kept = truncate_error_text(&body, MAX_PROVIDER_ERROR_BODY_CHARS);
		assert!(kept.starts_with("<html><html>"));
		assert!(kept.ends_with(&format!(
			"[truncated {} chars]",
			6_000_000 - MAX_PROVIDER_ERROR_BODY_CHARS
		)));
	}

	#[test]
	fn counts_utf16_code_units() {
		assert_eq!(truncate_error_text("😀😀", 3), "😀... [truncated 1 chars]");
		assert_eq!(truncate_error_text("😀😀", 4), "😀😀");
	}
}
