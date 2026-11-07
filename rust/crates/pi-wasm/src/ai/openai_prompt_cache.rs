//! Exports of `api/openai-prompt-cache`.

use pi_ai::api::openai_prompt_cache;
use wasm_bindgen::prelude::*;

/// `OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH`.
#[wasm_bindgen(js_name = aiOpenAIPromptCacheKeyMaxLength)]
pub fn openai_prompt_cache_key_max_length() -> usize {
	openai_prompt_cache::OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH
}

/// `clampOpenAIPromptCacheKey(key)`.
#[wasm_bindgen(js_name = aiClampOpenAIPromptCacheKey)]
pub fn clamp_openai_prompt_cache_key(key: Option<String>) -> Option<String> {
	openai_prompt_cache::clamp_openai_prompt_cache_key(key.as_deref())
}
