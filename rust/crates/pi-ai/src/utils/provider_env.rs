//! Port of `packages/ai/src/utils/provider-env.ts`.
//! PORT-STATUS: pending

use crate::types::ProviderEnv;

// PORT-STUB: w2-ai-core
/// Resolve a provider env value from scoped overrides, normal process.env, then
/// the duplicated Bun sandbox fallback for direct pi-ai consumers.
pub fn get_provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
	env.and_then(|env| env.get(name))
		.filter(|value| !value.is_empty())
		.cloned()
		.or_else(|| pi_host::env(name).filter(|value| !value.is_empty()))
}
