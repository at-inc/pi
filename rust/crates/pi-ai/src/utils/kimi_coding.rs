//! Port of `packages/ai/src/utils/kimi-coding.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Predicted by w2-ai-auth for `auth/oauth/kimi_coding.rs`; w2-ai-core's port replaces this stub.
use pi_host::PiResult;

use crate::auth::types::ProviderAuthInteraction;

/// The `{ oauthHost, baseUrl }` result of `kimiCodingEndpoints`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KimiCodingEndpoints {
	pub oauth_host: String,
	pub base_url: String,
}

/// Keep device authorization and Chat Completions requests in the same Kimi deployment. Fails like `new URL` on
/// an invalid host.
pub fn kimi_coding_endpoints(_oauth_host: Option<&str>, _base_url: Option<&str>) -> PiResult<KimiCodingEndpoints> {
	todo!("PORT-STUB")
}

/// Select and persist endpoints at login so later refreshes cannot switch account regions.
pub async fn select_kimi_coding_endpoints(_interaction: &ProviderAuthInteraction) -> PiResult<KimiCodingEndpoints> {
	todo!("PORT-STUB")
}
