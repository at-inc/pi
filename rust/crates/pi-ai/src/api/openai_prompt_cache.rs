//! Port of `packages/ai/src/api/openai-prompt-cache.ts`.
//! PORT-STATUS: reviewed
//!
//! `Array.from(key)` splits a string into code points, which is what `str::chars` yields.

pub const OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

pub fn clamp_openai_prompt_cache_key(key: Option<&str>) -> Option<String> {
	let key = key?;
	if key.chars().count() <= OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH {
		return Some(key.to_string());
	}
	Some(key.chars().take(OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH).collect())
}
