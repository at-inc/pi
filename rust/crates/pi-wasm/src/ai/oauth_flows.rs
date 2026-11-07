//! Exports of the per-provider OAuth flow modules (`auth/oauth/anthropic`, `github-copilot`, `kimi-coding`, `meta`,
//! `openai-chatgpt`, `openai-codex`, `openrouter`, `radius`, `xai`): each is one flow value, so they share this file.

use pi_ai::auth::oauth::radius::RadiusOAuthOptions;
use wasm_bindgen::prelude::*;

use super::auth::OAuthAuthHandle;
use crate::options::arg;

/// `anthropicOAuth`.
#[wasm_bindgen(js_name = aiAnthropicOAuth)]
pub fn anthropic_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::anthropic::anthropic_oauth())
}

/// `githubCopilotOAuth`.
#[wasm_bindgen(js_name = aiGithubCopilotOAuth)]
pub fn github_copilot_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::github_copilot::github_copilot_oauth())
}

/// `kimiCodingOAuth`.
#[wasm_bindgen(js_name = aiKimiCodingOAuth)]
pub fn kimi_coding_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::kimi_coding::kimi_coding_oauth())
}

/// `metaOAuth`.
#[wasm_bindgen(js_name = aiMetaOAuth)]
pub fn meta_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::meta::meta_oauth())
}

/// `openaiChatGPTOAuth`.
#[wasm_bindgen(js_name = aiOpenaiChatGPTOAuth)]
pub fn openai_chat_gpt_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::openai_chatgpt::openai_chat_gpt_oauth())
}

/// `openaiCodexOAuth`.
#[wasm_bindgen(js_name = aiOpenaiCodexOAuth)]
pub fn openai_codex_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::openai_codex::openai_codex_oauth())
}

/// `openRouterOAuth`.
#[wasm_bindgen(js_name = aiOpenRouterOAuth)]
pub fn open_router_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::openrouter::open_router_oauth())
}

/// `xaiOAuth`.
#[wasm_bindgen(js_name = aiXaiOAuth)]
pub fn xai_oauth() -> OAuthAuthHandle {
	OAuthAuthHandle::new(pi_ai::auth::oauth::xai::xai_oauth())
}

/// `createRadiusOAuth({ name, gateway })`.
#[wasm_bindgen(js_name = aiCreateRadiusOAuth)]
pub fn create_radius_oauth(options: JsValue) -> Result<OAuthAuthHandle, JsValue> {
	let field = |name: &str| js_sys::Reflect::get(&options, &name.into()).unwrap_or(JsValue::UNDEFINED);
	Ok(OAuthAuthHandle::new(pi_ai::auth::oauth::radius::create_radius_oauth(
		RadiusOAuthOptions {
			name: arg(field("name"))?,
			gateway: arg(field("gateway"))?,
		},
	)))
}
