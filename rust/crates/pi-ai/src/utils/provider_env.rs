//! Port of `packages/ai/src/utils/provider-env.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Signature copied from w2-ai-core's port on `capy/rust-port-w2-ai-core` (47702fe36), which replaces this stub.
use crate::types::ProviderEnv;

/// Resolve a provider env value from scoped overrides, then normal process.env.
pub fn get_provider_env_value(_name: &str, _env: Option<&ProviderEnv>) -> Option<String> {
	todo!("PORT-STUB")
}
