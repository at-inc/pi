//! Port of `packages/ai/src/api/openai-prompt-cache.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-openai-completions
// Predicted by w3-openai-responses for `openai_responses.rs`, `azure_openai_responses.rs` and
// `openai_codex_responses.rs`.

pub const OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

pub fn clamp_openai_prompt_cache_key(_key: Option<&str>) -> Option<String> {
	todo!("PORT-STUB")
}
