//! Port of `packages/ai/src/utils/provider-env.ts`.
//! PORT-STATUS: reviewed
//!
//! The Bun sandbox fallback (`getBunSandboxEnvValue`, a workaround for Bun compiled binaries with an empty
//! `process.env`) is not ported: the Rust core reads the environment through `pi_host::env`, whose embedding supplies
//! it. That branch never applies outside Bun.

use crate::types::ProviderEnv;

/// Resolve a provider env value from scoped overrides, then normal process.env.
pub fn get_provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
	if let Some(value) = env.and_then(|env| env.get(name))
		&& !value.is_empty()
	{
		return Some(value.clone());
	}
	pi_host::env(name).filter(|value| !value.is_empty())
}
