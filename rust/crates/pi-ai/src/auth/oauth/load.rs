//! Port of `packages/ai/src/auth/oauth/load.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-auth
use pi_host::PiResult;

use crate::auth::types::OAuthAuth;

pub async fn load_anthropic_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

pub async fn load_openai_codex_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

pub async fn load_openai_chatgpt_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

pub async fn load_github_copilot_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

pub async fn load_openrouter_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

pub async fn load_kimi_coding_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

pub async fn load_meta_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

pub async fn load_xai_oauth() -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}

/// `loadRadiusOAuth({ name, gateway })`.
pub async fn load_radius_oauth(_name: String, _gateway: String) -> PiResult<OAuthAuth> {
	todo!("PORT-STUB")
}
