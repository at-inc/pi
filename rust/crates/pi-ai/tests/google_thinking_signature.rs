//! Port of `packages/ai/test/google-thinking-signature.test.ts`.

use pi_ai::api::google_shared::{is_thinking_part, retain_thought_signature};
use serde_json::json;

mod google_thinking_detection_thought_signature {
	use super::*;

	#[test]
	fn treats_part_thought_true_as_thinking() {
		assert!(is_thinking_part(&json!({ "thought": true })));
		assert!(is_thinking_part(
			&json!({ "thought": true, "thoughtSignature": "opaque-signature" })
		));
	}

	#[test]
	fn does_not_treat_thought_signature_alone_as_thinking() {
		// Per Google docs, thoughtSignature is for context replay and can appear on any part type.
		// Only thought === true indicates thinking content.
		// See: https://ai.google.dev/gemini-api/docs/thought-signatures
		assert!(!is_thinking_part(&json!({ "thoughtSignature": "opaque-signature" })));
		assert!(!is_thinking_part(
			&json!({ "thought": false, "thoughtSignature": "opaque-signature" })
		));
	}

	#[test]
	fn does_not_treat_empty_missing_signatures_as_thinking_if_thought_is_not_set() {
		assert!(!is_thinking_part(&json!({})));
		assert!(!is_thinking_part(&json!({ "thought": false, "thoughtSignature": "" })));
	}

	#[test]
	fn preserves_the_existing_signature_when_subsequent_deltas_omit_thought_signature() {
		let first = retain_thought_signature(None, Some("sig-1"));
		assert_eq!(first.as_deref(), Some("sig-1"));

		let second = retain_thought_signature(first, None);
		assert_eq!(second.as_deref(), Some("sig-1"));

		let third = retain_thought_signature(second, Some(""));
		assert_eq!(third.as_deref(), Some("sig-1"));
	}

	#[test]
	fn updates_the_signature_when_a_new_non_empty_signature_arrives() {
		let updated = retain_thought_signature(Some("sig-1".into()), Some("sig-2"));
		assert_eq!(updated.as_deref(), Some("sig-2"));
	}
}
