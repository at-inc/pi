//! Port of `packages/ai/test/identity.test.ts`, the `client identity` block. The `client identity in requests` block
//! sends requests through `openai-completions` and `models.ts` (shard w2-ai-core) and moves with them.

use pi_ai::utils::identity::{
	ClientAttribution, ClientIdentity, ClientIdentityTarget, PI_ATTRIBUTION, UserAgentMode, UserAgentModeName,
	format_user_agent, get_attribution_headers, get_identity_headers, merge_client_identity,
};
use pi_ai::utils::version::PI_AI_VERSION;
use serde_json::{Value, json};

fn target<'a>(provider: &'a str, base_url: &'a str) -> ClientIdentityTarget<'a> {
	ClientIdentityTarget { provider, base_url }
}

fn acme(categories: Option<Vec<&str>>) -> ClientAttribution {
	ClientAttribution {
		name: "acme".into(),
		url: "https://acme.example".into(),
		categories: categories.map(|categories| categories.into_iter().map(String::from).collect()),
	}
}

fn attributed(attribution: ClientAttribution) -> ClientIdentity {
	ClientIdentity {
		attribution: Some(Some(attribution)),
		..ClientIdentity::default()
	}
}

fn headers_json(headers: indexmap::IndexMap<String, String>) -> Value {
	serde_json::to_value(headers).unwrap()
}

#[test]
fn keeps_the_version_constant_in_sync_with_package_json() {
	let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../packages/ai/package.json");
	let pkg: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
	assert_eq!(pkg["version"], PI_AI_VERSION);
}

#[test]
fn formats_user_agents() {
	let platform = match std::env::consts::OS {
		"macos" => "darwin",
		"windows" => "win32",
		other => other,
	};
	let arch = match std::env::consts::ARCH {
		"x86_64" => "x64",
		"aarch64" => "arm64",
		other => other,
	};
	let runtime = format!("rust/{}", env!("CARGO_PKG_RUST_VERSION"));
	assert_eq!(
		format_user_agent(None),
		format!("pi/{PI_AI_VERSION} ({platform}; {runtime}; {arch})")
	);
	let versioned = ClientIdentity {
		version: Some("9.9.9".into()),
		..ClientIdentity::default()
	};
	assert_eq!(
		format_user_agent(Some(&versioned)),
		format!("pi/9.9.9 ({platform}; {runtime}; {arch})")
	);
	let short = ClientIdentity {
		user_agent: Some(UserAgentMode::Named(UserAgentModeName::Short)),
		version: Some("9.9.9".into()),
		..ClientIdentity::default()
	};
	assert_eq!(format_user_agent(Some(&short)), "pi");
	let custom = ClientIdentity {
		user_agent: Some(UserAgentMode::Custom {
			custom: "acme/1.0".into(),
		}),
		..ClientIdentity::default()
	};
	assert_eq!(format_user_agent(Some(&custom)), "acme/1.0");
}

#[test]
fn merges_identities_field_by_field() {
	assert_eq!(merge_client_identity(&[None, None]), None);
	let base = ClientIdentity {
		attribution: Some(Some(PI_ATTRIBUTION.clone())),
		user_agent: Some(UserAgentMode::Named(UserAgentModeName::Short)),
		..ClientIdentity::default()
	};
	let versioned = ClientIdentity {
		version: Some("1.0.0".into()),
		..ClientIdentity::default()
	};
	assert_eq!(
		merge_client_identity(&[Some(&base), Some(&versioned)]),
		Some(ClientIdentity {
			attribution: Some(Some(PI_ATTRIBUTION.clone())),
			user_agent: Some(UserAgentMode::Named(UserAgentModeName::Short)),
			version: Some("1.0.0".into()),
		})
	);
	let off = ClientIdentity {
		attribution: Some(None),
		..ClientIdentity::default()
	};
	let merged = merge_client_identity(&[Some(&attributed(PI_ATTRIBUTION.clone())), Some(&off)]).unwrap();
	assert_eq!(serde_json::to_value(merged).unwrap(), json!({ "attribution": null }));
}

#[test]
fn parses_identities_from_json() {
	let identity: ClientIdentity =
		serde_json::from_value(json!({ "userAgent": { "custom": "acme" }, "attribution": null })).unwrap();
	assert_eq!(
		identity,
		ClientIdentity {
			user_agent: Some(UserAgentMode::Custom { custom: "acme".into() }),
			attribution: Some(None),
			version: None,
		}
	);
	let short: ClientIdentity = serde_json::from_value(json!({ "userAgent": "short" })).unwrap();
	assert_eq!(short.user_agent, Some(UserAgentMode::Named(UserAgentModeName::Short)));
	let unset: ClientIdentity = serde_json::from_value(json!({})).unwrap();
	assert_eq!(unset.attribution, None);
}

#[test]
fn sends_no_attribution_unless_enabled() {
	let open_router = target("openrouter", "https://openrouter.ai/api/v1");
	assert!(get_attribution_headers(open_router, None).is_empty());
	let off = ClientIdentity {
		attribution: Some(None),
		..ClientIdentity::default()
	};
	assert!(get_attribution_headers(open_router, Some(&off)).is_empty());
	assert_eq!(
		headers_json(get_identity_headers(open_router, None)),
		json!({ "User-Agent": format_user_agent(None) })
	);
}

#[test]
fn attributes_open_router_requests() {
	let open_router = target("openrouter", "https://openrouter.ai/api/v1");
	assert_eq!(
		headers_json(get_attribution_headers(
			open_router,
			Some(&attributed(PI_ATTRIBUTION.clone()))
		)),
		json!({
			"HTTP-Referer": "https://pi.dev",
			"X-OpenRouter-Title": "pi",
			"X-OpenRouter-Categories": "cli-agent",
		})
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			open_router,
			Some(&attributed(acme(Some(vec!["cli-agent", "cloud-agent"]))))
		)),
		json!({
			"HTTP-Referer": "https://acme.example",
			"X-OpenRouter-Title": "acme",
			"X-OpenRouter-Categories": "cli-agent,cloud-agent",
		})
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			open_router,
			Some(&attributed(acme(Some(vec![]))))
		)),
		json!({ "HTTP-Referer": "https://acme.example", "X-OpenRouter-Title": "acme" })
	);
	// Custom providers routed through OpenRouter, including legacy base URLs that are not valid URLs.
	let legacy = get_attribution_headers(
		target("custom", "not-a-url-openrouter.ai"),
		Some(&attributed(PI_ATTRIBUTION.clone())),
	);
	assert_eq!(legacy.get("X-OpenRouter-Title").map(String::as_str), Some("pi"));
}

#[test]
fn attributes_nvidia_nim_requests_with_a_capitalized_name_and_open_code_requests_with_the_name() {
	let pi = attributed(PI_ATTRIBUTION.clone());
	assert_eq!(
		headers_json(get_attribution_headers(
			target("nvidia", "https://example.test/v1"),
			Some(&pi)
		)),
		json!({ "X-BILLING-INVOKE-ORIGIN": "Pi" })
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			target("custom-nim", "https://integrate.api.nvidia.com/v1"),
			Some(&attributed(acme(None)))
		)),
		json!({ "X-BILLING-INVOKE-ORIGIN": "Acme" })
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			target("opencode", "https://opencode.ai/zen/v1"),
			Some(&pi)
		)),
		json!({ "x-opencode-client": "pi" })
	);
	assert_eq!(
		headers_json(get_attribution_headers(
			target("custom", "https://opencode.ai/zen/v1"),
			Some(&attributed(acme(None)))
		)),
		json!({ "x-opencode-client": "acme" })
	);
}

#[test]
fn does_not_attribute_nvidia_models_routed_through_gateways_to_nvidia() {
	let pi = attributed(PI_ATTRIBUTION.clone());
	assert!(
		!get_attribution_headers(target("openrouter", "https://openrouter.ai/api/v1"), Some(&pi))
			.contains_key("X-BILLING-INVOKE-ORIGIN")
	);
	assert!(
		get_attribution_headers(
			target("vercel-ai-gateway", "https://ai-gateway.vercel.sh/v1"),
			Some(&pi)
		)
		.is_empty()
	);
}
