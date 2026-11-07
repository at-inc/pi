//! Port of `packages/ai/src/utils/kimi-coding.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - The inline input and result types of `kimiCodingEndpoints` are named `KimiCodingEndpointsInput` and
//!   `KimiCodingEndpoints`.
//! - `new URL(value).hostname` throws a `TypeError` on an invalid URL, so `kimi_coding_endpoints` returns a
//!   `PiResult` with `TypeError: Invalid URL`.

use pi_host::{PiError, PiResult};

use crate::auth::types::{AuthPrompt, AuthPromptKind, AuthSelectOption, ProviderAuthInteraction};
use crate::utils::provider_env::get_provider_env_value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KimiCodingEndpointsInput {
	pub oauth_host: Option<String>,
	pub base_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KimiCodingEndpoints {
	pub oauth_host: String,
	pub base_url: String,
}

fn hostname(value: &str) -> PiResult<String> {
	let url = url::Url::parse(value).map_err(|_| PiError::type_error("Invalid URL"))?;
	Ok(url.host_str().unwrap_or_default().to_string())
}

fn non_empty(value: &Option<String>) -> Option<&str> {
	value.as_deref().filter(|value| !value.is_empty())
}

/// Keep device authorization and Chat Completions requests in the same Kimi deployment.
pub fn kimi_coding_endpoints(input: KimiCodingEndpointsInput) -> PiResult<KimiCodingEndpoints> {
	let oauth_host = non_empty(&input.oauth_host);
	let base_url = non_empty(&input.base_url);
	let global = match (oauth_host, base_url) {
		(Some(oauth_host), _) => hostname(oauth_host)? == "auth.kimi.ai",
		(None, Some(base_url)) => hostname(base_url)? == "api.kimi.ai",
		(None, None) => false,
	};
	let domain = if global { "kimi.ai" } else { "kimi.com" };
	let oauth_host = oauth_host
		.map(str::to_string)
		.unwrap_or_else(|| format!("https://auth.{domain}"));
	let base_url = base_url
		.map(str::to_string)
		.unwrap_or_else(|| format!("https://api.{domain}/coding"));
	let mut base_url = base_url.trim_end_matches('/');
	while let Some(stripped) = base_url.strip_suffix("/v1") {
		base_url = stripped;
	}
	Ok(KimiCodingEndpoints {
		oauth_host: oauth_host.trim_end_matches('/').to_string(),
		base_url: format!("{base_url}/v1"),
	})
}

/// Select and persist endpoints at login so later refreshes cannot switch account regions.
pub async fn select_kimi_coding_endpoints(interaction: &ProviderAuthInteraction) -> PiResult<KimiCodingEndpoints> {
	interaction.signal.throw_if_aborted()?;
	let oauth_host = get_provider_env_value("KIMI_CODE_OAUTH_HOST", None)
		.or_else(|| get_provider_env_value("KIMI_OAUTH_HOST", None));
	let base_url = get_provider_env_value("KIMI_CODE_BASE_URL", None);
	if oauth_host.is_some() || base_url.is_some() {
		return kimi_coding_endpoints(KimiCodingEndpointsInput { oauth_host, base_url });
	}

	let region = (interaction.prompt)(AuthPrompt::new(AuthPromptKind::Select {
		message: "Select your Kimi account region:".into(),
		options: vec![
			AuthSelectOption {
				id: "global".into(),
				label: "International (kimi.ai)".into(),
				description: Some("Includes Google sign-in".into()),
			},
			AuthSelectOption {
				id: "mainland-cn".into(),
				label: "Mainland China (kimi.com)".into(),
				description: None,
			},
		],
	}))
	.await?;
	interaction.signal.throw_if_aborted()?;
	if region != "global" && region != "mainland-cn" {
		return Err(PiError::new(format!("Unknown Kimi account region: {region}")));
	}
	kimi_coding_endpoints(KimiCodingEndpointsInput {
		oauth_host: Some(format!(
			"https://auth.kimi.{}",
			if region == "global" { "ai" } else { "com" }
		)),
		base_url: None,
	})
}
