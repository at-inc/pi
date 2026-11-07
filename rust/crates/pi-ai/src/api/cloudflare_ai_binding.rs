//! Port of `packages/ai/src/api/cloudflare-ai-binding.ts`.
//! PORT-STATUS: ported
//!
//! AI Gateway transport over the Workers AI binding.
//!
//! pi's Cloudflare AI Gateway support speaks HTTPS
//! (`gateway.ai.cloudflare.com/v1/{account}/{gateway}/{provider}/...`, see `api/cloudflare.rs`), which needs a
//! Cloudflare API token even when the caller is a Worker in the gateway's own account.
//!
//! A Worker avoids that token by talking to the gateway through the AI binding's `fetch` passthrough
//! (`env.AI.fetch()`), which serves the gateway's provider passthrough at
//! `https://workers-binding.ai/ai-gateway/gateways/{gateway}/{provider}/{endpoint...}` — the same shape as the HTTPS
//! URL, minus the account id (the binding channel carries identity). Binding calls are pre-authenticated in-account
//! and return the provider's native wire format as a regular (streaming) response, so API implementations behave
//! identically over either transport.
//!
//! A model whose `base_url` names that route therefore needs no translation: point it there and pass
//! [`create_ai_binding_fetch`] as the request `fetch`. Nothing is rewritten, buffered or re-encoded.
//!
//! In Rust the binding is host-supplied: the wasm binding wraps `env.AI` in an [`AiBinding`] whose `fetch` calls
//! `Ai#fetch`. `fetch` stays optional because the published `Ai` type does not declare it, and the check runs once
//! at construction as in the TS.

use pi_host::{FetchFunction, PiError, PiResult};

/// The Workers AI binding (`env.AI`), described structurally.
///
/// `fetch` is optional only because the published `Ai` type doesn't declare it yet — every real binding has it at
/// runtime. `ai_gateway_log_id` pins the type to the AI binding: it is the one member unique to `Ai`.
#[derive(Clone, Default)]
pub struct AiBinding {
	pub ai_gateway_log_id: Option<String>,
	pub fetch: Option<FetchFunction>,
}

/// Placeholder value for auth headers on binding-routed requests. API implementations require an API key or a
/// recognized auth header (`authorization`, `x-api-key`, `cf-aig-authorization`) before dispatch; binding calls are
/// pre-authenticated, so pass `cf-aig-authorization: Bearer {CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL}` to satisfy
/// the check. The gateway ignores (and strips) `cf-aig-authorization` on binding-routed requests. Pair it with
/// `Authorization: null` / `x-api-key: null` so the SDKs' placeholder auth headers never reach the gateway, which
/// would treat a request-supplied auth header as a BYOK provider key that overrides its stored keys.
pub const CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL: &str = "cloudflare-gateway-binding";

/// Create a `fetch` backed by the AI binding, for models whose `base_url` already names a route the binding serves —
/// including the gateway's provider passthrough,
/// `https://workers-binding.ai/ai-gateway/gateways/{gateway}/{provider}/...`. Requests pass through untouched.
pub fn create_ai_binding_fetch(binding: &AiBinding) -> PiResult<FetchFunction> {
	// `fetch` is optional on the type, so its presence is checked here — early, rather than as a confusing failure
	// on the first inference request.
	let Some(binding_fetch) = binding.fetch.clone() else {
		return Err(PiError::type_error(
			"createAiBindingFetch: the AI binding does not expose fetch()",
		));
	};
	Ok(binding_fetch)
}
