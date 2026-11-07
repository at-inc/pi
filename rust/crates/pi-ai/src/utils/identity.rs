//! Port of `packages/ai/src/utils/identity.ts`.
//! PORT-STATUS: ported
//!
//! The runtime in the detailed `User-Agent` comes from the Rust build: `rust/<rust-version>` with Node-style platform
//! and architecture names, and `(browser)` on wasm32, where TS reads `process` (`node/v24.x` or `bun/1.x`).
//! `withClientIdentity` wraps the `Models` interface of `models.ts` (shard w2-ai-core) and is not ported here; see the
//! PORT-TODO at the end of the file.

use std::sync::LazyLock;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::version::PI_AI_VERSION;

/// The `User-Agent` sent to providers:
/// - `"detailed"`: `pi/<version> (<platform>; <runtime>/<runtime version>; <arch>)`, or `pi/<version> (browser)`
///   outside Node.js and Bun.
/// - `"short"`: `pi`.
/// - `{ custom }`: the given value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserAgentMode {
	Named(UserAgentModeName),
	Custom { custom: String },
}

/// The named `User-Agent` modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserAgentModeName {
	#[serde(rename = "short")]
	Short,
	#[serde(rename = "detailed")]
	Detailed,
}

/// Who requests are attributed to. `getAttributionHeaders()` maps the fields to each provider's headers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientAttribution {
	/// App name: OpenRouter app title, NVIDIA invoke origin (first letter upper-cased), OpenCode client.
	pub name: String,
	/// App URL: OpenRouter `HTTP-Referer`, the app's identity in OpenRouter's rankings, grouped by origin.
	pub url: String,
	/// App categories, e.g. `["cli-agent"]`: OpenRouter `X-OpenRouter-Categories`, comma-separated.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub categories: Option<Vec<String>>,
}

/// pi's attribution, used by pi-agent-core, pi-durable, and the coding agent.
pub static PI_ATTRIBUTION: LazyLock<ClientAttribution> = LazyLock::new(|| ClientAttribution {
	name: "pi".into(),
	url: "https://pi.dev".into(),
	categories: Some(vec!["cli-agent".into()]),
});

/// How pi-ai identifies the calling app to providers. Every field is optional; absent fields use pi-ai's defaults:
/// a detailed `User-Agent` with pi-ai's version and no attribution headers. Plain JSON, so it can be persisted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIdentity {
	/// Default: `"detailed"`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub user_agent: Option<UserAgentMode>,
	/// Provider attribution headers. Absent or `null`: none; `null` also turns off attribution set by a merged identity.
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub attribution: Option<Option<ClientAttribution>>,
	/// Version in the detailed `User-Agent`. Default: the pi-ai version.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub version: Option<String>,
}

/// The provider and endpoint a request goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientIdentityTarget<'a> {
	pub provider: &'a str,
	pub base_url: &'a str,
}

const NVIDIA_NIM_HOST: &str = "integrate.api.nvidia.com";
const OPENCODE_HOST: &str = "opencode.ai";

/// Merge identities field by field; later sources win and undefined fields are skipped.
pub fn merge_client_identity(sources: &[Option<&ClientIdentity>]) -> Option<ClientIdentity> {
	let mut merged: Option<ClientIdentity> = None;
	for source in sources {
		let Some(source) = source else { continue };
		let mut next = merged.unwrap_or_default();
		if source.user_agent.is_some() {
			next.user_agent = source.user_agent.clone();
		}
		if source.attribution.is_some() {
			next.attribution = source.attribution.clone();
		}
		if source.version.is_some() {
			next.version = source.version.clone();
		}
		merged = Some(next);
	}
	merged
}

/// The version in the detailed `User-Agent`.
pub fn get_client_version(identity: Option<&ClientIdentity>) -> String {
	identity
		.and_then(|identity| identity.version.clone())
		.unwrap_or_else(|| PI_AI_VERSION.to_string())
}

fn runtime_description() -> Option<String> {
	if cfg!(target_arch = "wasm32") {
		return None;
	}
	Some(format!("rust/{}", env!("CARGO_PKG_RUST_VERSION")))
}

fn platform() -> &'static str {
	match std::env::consts::OS {
		"macos" => "darwin",
		"windows" => "win32",
		other => other,
	}
}

fn arch() -> &'static str {
	match std::env::consts::ARCH {
		"x86_64" => "x64",
		"aarch64" => "arm64",
		"x86" => "ia32",
		"powerpc64" => "ppc64",
		other => other,
	}
}

/// The `User-Agent` value of an identity.
pub fn format_user_agent(identity: Option<&ClientIdentity>) -> String {
	let mode = identity
		.and_then(|identity| identity.user_agent.clone())
		.unwrap_or(UserAgentMode::Named(UserAgentModeName::Detailed));
	match mode {
		UserAgentMode::Custom { custom } => return custom,
		UserAgentMode::Named(UserAgentModeName::Short) => return "pi".into(),
		UserAgentMode::Named(UserAgentModeName::Detailed) => {}
	}
	let product = format!("pi/{}", get_client_version(identity));
	match runtime_description() {
		Some(runtime) if !runtime.is_empty() => format!("{product} ({}; {runtime}; {})", platform(), arch()),
		_ => format!("{product} (browser)"),
	}
}

fn matches_host(base_url: &str, expected_host: &str) -> bool {
	match url::Url::parse(base_url) {
		Ok(url) => url.host_str() == Some(expected_host),
		Err(_) => false,
	}
}

fn is_open_router(target: ClientIdentityTarget) -> bool {
	target.provider == "openrouter" || target.base_url.contains("openrouter.ai")
}

fn is_nvidia_nim(target: ClientIdentityTarget) -> bool {
	target.provider == "nvidia" || matches_host(target.base_url, NVIDIA_NIM_HOST)
}

fn is_open_code(target: ClientIdentityTarget) -> bool {
	target.provider == "opencode" || target.provider == "opencode-go" || matches_host(target.base_url, OPENCODE_HOST)
}

/// Provider attribution headers of an identity for a request target. Empty when attribution is off.
pub fn get_attribution_headers(
	target: ClientIdentityTarget,
	identity: Option<&ClientIdentity>,
) -> IndexMap<String, String> {
	let mut headers = IndexMap::new();
	let Some(Some(attribution)) = identity
		.and_then(|identity| identity.attribution.as_ref())
		.map(Option::as_ref)
	else {
		return headers;
	};
	let ClientAttribution { name, url, categories } = attribution;
	if is_open_router(target) {
		headers.insert("HTTP-Referer".into(), url.clone());
		headers.insert("X-OpenRouter-Title".into(), name.clone());
		if let Some(categories) = categories
			&& !categories.is_empty()
		{
			headers.insert("X-OpenRouter-Categories".into(), categories.join(","));
		}
		return headers;
	}
	// NVIDIA expects a capitalized origin, such as `Pi`.
	if is_nvidia_nim(target) {
		headers.insert("X-BILLING-INVOKE-ORIGIN".into(), capitalize_first(name));
		return headers;
	}
	if is_open_code(target) {
		headers.insert("x-opencode-client".into(), name.clone());
	}
	headers
}

/// `name.charAt(0).toUpperCase() + name.slice(1)`. A first character outside the BMP starts with a surrogate, which
/// `toUpperCase` leaves unchanged, so the name stays as it is.
fn capitalize_first(name: &str) -> String {
	let mut chars = name.chars();
	match chars.next() {
		Some(first) if first.len_utf16() == 1 => first.to_uppercase().collect::<String>() + chars.as_str(),
		_ => name.to_string(),
	}
}

/// OpenCode's per-conversation routing header for requests to OpenCode, including custom providers pointing at it.
/// Undefined without a session or for other targets.
pub fn get_open_code_session_headers(
	target: ClientIdentityTarget,
	session_id: Option<&str>,
) -> Option<IndexMap<String, String>> {
	match session_id {
		Some(session_id) if !session_id.is_empty() && is_open_code(target) => Some(IndexMap::from([(
			"x-opencode-session".to_string(),
			session_id.to_string(),
		)])),
		_ => None,
	}
}

/// Default headers an identity adds to a request: `User-Agent` and provider attribution. APIs apply them below model,
/// provider, and request headers, so configured headers override them and `null` request headers remove them.
pub fn get_identity_headers(
	target: ClientIdentityTarget,
	identity: Option<&ClientIdentity>,
) -> IndexMap<String, String> {
	let mut headers = IndexMap::from([("User-Agent".to_string(), format_user_agent(identity))]);
	headers.extend(get_attribution_headers(target, identity));
	headers
}

// PORT-TODO: w2-ai-core: `withClientIdentity(models, identity)` wraps `Models` / `MutableModels` from `models.ts`. It
// returns a `Models` that delegates every method and merges `identity` under each request's `options.identity` with
// `merge_client_identity(&[Some(identity), options.identity.as_ref()])` for `stream`, `complete`, `streamSimple`,
// `completeSimple`, `streamDeferred`, `fetchDeferred`, `cancelDeferred`, `generateImages`, and `classify`, and keeps
// `setProvider`/`deleteProvider`/`clearProviders` when `models` is mutable. Port it next to the `Models` trait.
