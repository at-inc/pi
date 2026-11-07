//! Port of `packages/ai/src/utils/sanitize-unicode.ts`.
//! PORT-STATUS: ported
//!
//! A Rust `str` is valid UTF-8 and cannot hold an unpaired surrogate, so there is nothing to remove: text decoded
//! from UTF-16 or JSON in this crate already has U+FFFD in place of an unpaired surrogate.

/// Removes unpaired Unicode surrogate characters from a string.
///
/// Unpaired surrogates (high surrogates 0xD800-0xDBFF without matching low surrogates 0xDC00-0xDFFF,
/// or vice versa) cause JSON serialization errors in many API providers.
///
/// Valid emoji and other characters outside the Basic Multilingual Plane use properly paired
/// surrogates and will NOT be affected by this function.
pub fn sanitize_surrogates(text: &str) -> &str {
	text
}
