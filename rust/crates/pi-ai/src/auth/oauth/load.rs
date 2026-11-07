//! Port of `packages/ai/src/auth/oauth/load.ts`.
//! PORT-STATUS: ported
//!
//! Port notes: TS imports each flow module through a variable specifier so bundlers cannot follow the import into
//! Node-only flow code. Rust links every flow, so a loader calls the flow's constructor directly (the Node-only
//! parts sit behind the `native` feature), and the specifier rewrite has no counterpart. The loaders stay `async`
//! and fallible like the TS promises, so callers keep the lazy-load shape. A bundled loader returns the flow
//! synchronously; TS also accepts a promise there, which no registrant uses. The registered loaders are per thread,
//! like the per-isolate module state in TS (`OAuthAuth` holds `Rc` closures).

use std::cell::RefCell;
use std::rc::Rc;

use pi_host::PiResult;

use crate::auth::oauth::anthropic::anthropic_oauth;
use crate::auth::oauth::github_copilot::github_copilot_oauth;
use crate::auth::oauth::kimi_coding::kimi_coding_oauth;
use crate::auth::oauth::meta::meta_oauth;
use crate::auth::oauth::openai_chatgpt::openai_chat_gpt_oauth;
use crate::auth::oauth::openai_codex::openai_codex_oauth;
use crate::auth::oauth::openrouter::open_router_oauth;
use crate::auth::oauth::radius::{RadiusOAuthOptions, create_radius_oauth};
use crate::auth::oauth::xai::xai_oauth;
use crate::auth::types::OAuthAuth;

/// A loader of one OAuth flow.
pub type OAuthFlowLoader = Rc<dyn Fn() -> OAuthAuth>;

/// `OAuthFlowLoaders`.
#[derive(Clone)]
pub struct OAuthFlowLoaders {
	pub anthropic: OAuthFlowLoader,
	pub openai_codex: OAuthFlowLoader,
	pub openai_chat_gpt: OAuthFlowLoader,
	pub github_copilot: OAuthFlowLoader,
	pub openrouter: OAuthFlowLoader,
	pub kimi_coding: OAuthFlowLoader,
	pub meta: OAuthFlowLoader,
	pub xai: OAuthFlowLoader,
	pub radius: Rc<dyn Fn(RadiusOAuthOptions) -> OAuthAuth>,
}

thread_local! {
	static BUNDLED_LOADERS: RefCell<Option<OAuthFlowLoaders>> = const { RefCell::new(None) };
}

/// Registers statically bundled OAuth flows for standalone Bun binaries.
pub fn register_bundled_oauth_flow_loaders(loaders: OAuthFlowLoaders) {
	BUNDLED_LOADERS.with(|bundled| *bundled.borrow_mut() = Some(loaders));
}

fn bundled_loaders() -> Option<OAuthFlowLoaders> {
	BUNDLED_LOADERS.with(|bundled| bundled.borrow().clone())
}

/// `loadAnthropicOAuth`.
pub async fn load_anthropic_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.anthropic)());
	}
	Ok(anthropic_oauth())
}

/// `loadOpenAICodexOAuth`.
pub async fn load_openai_codex_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.openai_codex)());
	}
	Ok(openai_codex_oauth())
}

/// `loadOpenAIChatGPTOAuth`.
pub async fn load_openai_chatgpt_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.openai_chat_gpt)());
	}
	Ok(openai_chat_gpt_oauth())
}

/// `loadGitHubCopilotOAuth`.
pub async fn load_github_copilot_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.github_copilot)());
	}
	Ok(github_copilot_oauth())
}

/// `loadOpenRouterOAuth`.
pub async fn load_openrouter_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.openrouter)());
	}
	Ok(open_router_oauth())
}

/// `loadKimiCodingOAuth`.
pub async fn load_kimi_coding_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.kimi_coding)());
	}
	Ok(kimi_coding_oauth())
}

/// `loadMetaOAuth`.
pub async fn load_meta_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.meta)());
	}
	Ok(meta_oauth())
}

/// `loadXaiOAuth`.
pub async fn load_xai_oauth() -> PiResult<OAuthAuth> {
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.xai)());
	}
	Ok(xai_oauth())
}

/// `loadRadiusOAuth({ name, gateway })`.
pub async fn load_radius_oauth(name: String, gateway: String) -> PiResult<OAuthAuth> {
	let options = RadiusOAuthOptions { name, gateway };
	if let Some(loaders) = bundled_loaders() {
		return Ok((loaders.radius)(options));
	}
	Ok(create_radius_oauth(options))
}
