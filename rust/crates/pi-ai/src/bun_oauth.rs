//! Port of `packages/ai/src/bun-oauth.ts`.
//! PORT-STATUS: reviewed

use std::rc::Rc;

use crate::auth::oauth::anthropic::anthropic_oauth;
use crate::auth::oauth::github_copilot::github_copilot_oauth;
use crate::auth::oauth::kimi_coding::kimi_coding_oauth;
use crate::auth::oauth::load::{OAuthFlowLoaders, register_bundled_oauth_flow_loaders};
use crate::auth::oauth::meta::meta_oauth;
use crate::auth::oauth::openai_chatgpt::openai_chat_gpt_oauth;
use crate::auth::oauth::openai_codex::openai_codex_oauth;
use crate::auth::oauth::openrouter::open_router_oauth;
use crate::auth::oauth::radius::create_radius_oauth;
use crate::auth::oauth::xai::xai_oauth;

/// Register OAuth flows statically embedded in the standalone Bun binary.
pub fn register_bun_oauth_flows() {
	register_bundled_oauth_flow_loaders(OAuthFlowLoaders {
		anthropic: Rc::new(anthropic_oauth),
		openai_codex: Rc::new(openai_codex_oauth),
		openai_chat_gpt: Rc::new(openai_chat_gpt_oauth),
		github_copilot: Rc::new(github_copilot_oauth),
		openrouter: Rc::new(open_router_oauth),
		kimi_coding: Rc::new(kimi_coding_oauth),
		meta: Rc::new(meta_oauth),
		xai: Rc::new(xai_oauth),
		radius: Rc::new(create_radius_oauth),
	});
}
