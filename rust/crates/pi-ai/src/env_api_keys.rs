//! Port of `packages/ai/src/env-api-keys.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Signatures copied from w2-ai-core's port on `capy/rust-port-w2-ai-core` (c990cc571), which replaces this stub.
use crate::types::ProviderEnv;

pub fn get_env_api_key(_provider: &str, _env: Option<&ProviderEnv>) -> Option<String> {
	todo!("PORT-STUB")
}
