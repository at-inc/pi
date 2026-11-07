//! Port of `packages/ai/src/oauth.ts`.
//! PORT-STATUS: ported

/// Type-only compatibility entry point for coding-agent extension OAuth declarations.
pub use crate::compat::extension_oauth_types::{
	OAuthAuthInfo, OAuthCredentials, OAuthDeviceCodeInfo, OAuthLoginCallbacks, OAuthPrompt, OAuthSelectOption,
	OAuthSelectPrompt,
};
