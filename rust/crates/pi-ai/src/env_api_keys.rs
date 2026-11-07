//! Port of `packages/ai/src/env-api-keys.ts`.
//! PORT-STATUS: ported
//!
//! The TS loads `node:fs`, `node:os` and `node:path` lazily and treats a runtime without them as a browser. The
//! port reads the file system through the host (`pi_host::file_exists`, `pi_host::home_dir`): a host without a home
//! directory is a browser, whose missing ADC file is cached permanently. The cache is per thread. The TS
//! overloads for `KnownProvider` and `string` are one function taking `&str`.

use std::cell::Cell;
use std::path::Path;

use crate::types::ProviderEnv;
use crate::utils::provider_env::get_provider_env_value;

pub const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
pub const ANTHROPIC_OAUTH_TOKEN_ENV: &str = "ANTHROPIC_OAUTH_TOKEN";
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const ANTHROPIC_FEDERATION_RULE_ID_ENV: &str = "ANTHROPIC_FEDERATION_RULE_ID";
pub const ANTHROPIC_ORGANIZATION_ID_ENV: &str = "ANTHROPIC_ORGANIZATION_ID";
pub const ANTHROPIC_SERVICE_ACCOUNT_ID_ENV: &str = "ANTHROPIC_SERVICE_ACCOUNT_ID";
pub const ANTHROPIC_IDENTITY_TOKEN_FILE_ENV: &str = "ANTHROPIC_IDENTITY_TOKEN_FILE";
pub const ANTHROPIC_WORKSPACE_ID_ENV: &str = "ANTHROPIC_WORKSPACE_ID";

thread_local! {
	static CACHED_VERTEX_ADC_CREDENTIALS_EXISTS: Cell<Option<bool>> = const { Cell::new(None) };
}

fn has_vertex_adc_credentials(env: Option<&ProviderEnv>) -> bool {
	if let Some(explicit_credentials_path) = env
		.and_then(|env| env.get("GOOGLE_APPLICATION_CREDENTIALS"))
		.filter(|path| !path.is_empty())
	{
		return pi_host::file_exists(explicit_credentials_path);
	}

	if let Some(cached) = CACHED_VERTEX_ADC_CREDENTIALS_EXISTS.get() {
		return cached;
	}
	let Some(home_dir) = pi_host::home_dir() else {
		// Definitively in a browser — safe to cache false permanently
		CACHED_VERTEX_ADC_CREDENTIALS_EXISTS.set(Some(false));
		return false;
	};

	// Check GOOGLE_APPLICATION_CREDENTIALS env var first (standard way)
	let exists = match get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env) {
		Some(gac_path) => pi_host::file_exists(&gac_path),
		// Fall back to default ADC path (lazy evaluation)
		None => pi_host::file_exists(
			&Path::new(&home_dir)
				.join(".config")
				.join("gcloud")
				.join("application_default_credentials.json")
				.to_string_lossy(),
		),
	};
	CACHED_VERTEX_ADC_CREDENTIALS_EXISTS.set(Some(exists));
	exists
}

fn get_api_key_env_vars(provider: &str) -> Option<Vec<&'static str>> {
	if provider == "github-copilot" {
		return Some(vec!["COPILOT_GITHUB_TOKEN"]);
	}

	// ANTHROPIC_AUTH_TOKEN participates in env discovery/status, but
	// getEnvApiKey() skips it because requests must pass it as Authorization: Bearer.
	if provider == "anthropic" {
		return Some(vec![
			ANTHROPIC_AUTH_TOKEN_ENV,
			ANTHROPIC_OAUTH_TOKEN_ENV,
			ANTHROPIC_API_KEY_ENV,
		]);
	}

	let env_var = match provider {
		"ant-ling" => "ANT_LING_API_KEY",
		"qwen-token-plan" => "QWEN_TOKEN_PLAN_API_KEY",
		"qwen-token-plan-cn" => "QWEN_TOKEN_PLAN_CN_API_KEY",
		"qwen-token-plan-individual" => "QWEN_TOKEN_PLAN_API_KEY",
		"openai" => "OPENAI_API_KEY",
		"azure" => "AZURE_OPENAI_API_KEY",
		"nvidia" => "NVIDIA_API_KEY",
		"deepseek" => "DEEPSEEK_API_KEY",
		"google" => "GEMINI_API_KEY",
		"google-vertex" => "GOOGLE_CLOUD_API_KEY",
		"groq" => "GROQ_API_KEY",
		"cerebras" => "CEREBRAS_API_KEY",
		"xai" => "XAI_API_KEY",
		"typesafe" => "TYPESAFE_API_KEY",
		"radius" => "RADIUS_API_KEY",
		"openrouter" => "OPENROUTER_API_KEY",
		"vercel-ai-gateway" => "AI_GATEWAY_API_KEY",
		"zai" => "ZAI_API_KEY",
		"zai-coding-cn" => "ZAI_CODING_CN_API_KEY",
		"mistral" => "MISTRAL_API_KEY",
		"minimax" => "MINIMAX_API_KEY",
		"minimax-cn" => "MINIMAX_CN_API_KEY",
		"moonshotai" => "MOONSHOT_API_KEY",
		"moonshotai-cn" => "MOONSHOT_API_KEY",
		"huggingface" => "HF_TOKEN",
		"fireworks" => "FIREWORKS_API_KEY",
		"together" => "TOGETHER_API_KEY",
		"baseten" => "BASETEN_API_KEY",
		"opencode" => "OPENCODE_API_KEY",
		"opencode-go" => "OPENCODE_API_KEY",
		"kimi-coding" => "KIMI_API_KEY",
		"meta" => "META_API_KEY",
		"cloudflare-workers-ai" => "CLOUDFLARE_API_KEY",
		"cloudflare-ai-gateway" => "CLOUDFLARE_API_KEY",
		"xiaomi" => "XIAOMI_API_KEY",
		"xiaomi-token-plan-cn" => "XIAOMI_TOKEN_PLAN_CN_API_KEY",
		"xiaomi-token-plan-ams" => "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
		"xiaomi-token-plan-sgp" => "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
		_ => return None,
	};
	Some(vec![env_var])
}

/// Find configured environment variables that can provide an API key for a provider.
///
/// This only reports actual API key variables. It intentionally excludes ambient
/// credential sources such as AWS profiles, AWS IAM credentials, and Google
/// Application Default Credentials.
pub fn find_env_keys(provider: &str, env: Option<&ProviderEnv>) -> Option<Vec<String>> {
	let env_vars = get_api_key_env_vars(provider)?;

	let found: Vec<String> = env_vars
		.into_iter()
		.filter(|env_var| get_provider_env_value(env_var, env).is_some())
		.map(str::to_string)
		.collect();
	if found.is_empty() { None } else { Some(found) }
}

/// Get API key for provider from known environment variables, e.g. OPENAI_API_KEY.
///
/// Will not return API keys for providers that require OAuth tokens.
pub fn get_env_api_key(provider: &str, env: Option<&ProviderEnv>) -> Option<String> {
	if let Some(env_keys) = find_env_keys(provider, env).filter(|keys| !keys[0].is_empty()) {
		let api_key_env = if provider == "anthropic" {
			env_keys.iter().find(|key| *key != ANTHROPIC_AUTH_TOKEN_ENV)
		} else {
			env_keys.first()
		};
		if let Some(api_key_env) = api_key_env {
			return get_provider_env_value(api_key_env, env);
		}
	}

	// Vertex AI supports either an explicit API key or Application Default Credentials.
	// Auth is configured via `gcloud auth application-default login`.
	if provider == "google-vertex" {
		let has_credentials = has_vertex_adc_credentials(env);
		let has_project = get_provider_env_value("GOOGLE_CLOUD_PROJECT", env).is_some()
			|| get_provider_env_value("GCLOUD_PROJECT", env).is_some();
		let has_location = get_provider_env_value("GOOGLE_CLOUD_LOCATION", env).is_some();

		if has_credentials && has_project && has_location {
			return Some("<authenticated>".into());
		}
	}

	if provider == "amazon-bedrock" {
		// Amazon Bedrock supports multiple credential sources:
		// 1. AWS_PROFILE - named profile from ~/.aws/credentials
		// 2. AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY - standard IAM keys
		// 3. AWS_BEARER_TOKEN_BEDROCK - Bedrock bearer token
		// 4. AWS_CONTAINER_CREDENTIALS_RELATIVE_URI - ECS task roles
		// 5. AWS_CONTAINER_CREDENTIALS_FULL_URI - ECS task roles (full URI)
		// 6. AWS_WEB_IDENTITY_TOKEN_FILE - IRSA (IAM Roles for Service Accounts)
		let set = |name: &str| get_provider_env_value(name, env).is_some();
		if set("AWS_PROFILE")
			|| (set("AWS_ACCESS_KEY_ID") && set("AWS_SECRET_ACCESS_KEY"))
			|| set("AWS_BEARER_TOKEN_BEDROCK")
			|| set("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
			|| set("AWS_CONTAINER_CREDENTIALS_FULL_URI")
			|| set("AWS_WEB_IDENTITY_TOKEN_FILE")
		{
			return Some("<authenticated>".into());
		}
	}

	None
}
