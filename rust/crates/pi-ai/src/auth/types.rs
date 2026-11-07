//! Port of `packages/ai/src/auth/types.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-auth
// Predicted shapes that w2-ai-core (models.rs, providers) builds on. Interfaces implemented by apps
// (`CredentialStore`, `AuthContext`) are traits; object-literal interfaces with methods (`ApiKeyAuth`,
// `OAuthAuth`, `AuthInteraction`) are structs of `Rc` callbacks, with optional methods as `Option` fields.

use std::rc::Rc;

use pi_host::{AbortSignal, LocalBoxFuture, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::types::{ProviderEnv, ProviderHeaders};

/// Request auth for a single model request. If a value cannot be expressed as
/// `apiKey`, `headers`, or `baseUrl`, it is provider config, not auth.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelAuth {
	pub api_key: Option<String>,
	pub headers: Option<ProviderHeaders>,
	pub base_url: Option<String>,
}

/// Stored api-key credential. `env` holds provider-scoped environment/config
/// values such as Cloudflare account/gateway ids.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyCredential {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub key: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub env: Option<ProviderEnv>,
}

/// OAuth token data returned by extension compatibility flows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCredentials {
	pub refresh: String,
	pub access: String,
	pub expires: f64,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

/// Stored canonical OAuth credential.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCredential {
	pub refresh: String,
	pub access: String,
	pub expires: f64,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

/// One type-tagged credential per provider — the shape of today's auth.json.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credential {
	#[serde(rename = "api_key")]
	ApiKey(ApiKeyCredential),
	#[serde(rename = "oauth")]
	OAuth(OAuthCredential),
}

impl Credential {
	/// `credential.type`.
	pub fn r#type(&self) -> AuthType {
		match self {
			Credential::ApiKey(_) => AuthType::ApiKey,
			Credential::OAuth(_) => AuthType::OAuth,
		}
	}
}

/// Non-secret credential metadata for account/status enumeration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialInfo {
	pub provider_id: String,
	#[serde(rename = "type")]
	pub r#type: AuthType,
}

/// Optional cancellation for public auth and credential operations.
#[derive(Clone, Debug, Default)]
pub struct AuthOperationOptions {
	pub signal: Option<AbortSignal>,
}

/// `CredentialStore.modify`'s callback.
pub type CredentialModifier =
	Box<dyn FnOnce(Option<Credential>) -> LocalBoxFuture<'static, PiResult<Option<Credential>>>>;

/// App-owned credential storage, keyed by `Provider.id`, one credential per provider.
pub trait CredentialStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>>;
	fn list(&self, options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>>;
	fn modify(
		&self,
		provider_id: &str,
		f: CredentialModifier,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>>;
	fn delete(&self, provider_id: &str, options: Option<AuthOperationOptions>)
	-> LocalBoxFuture<'static, PiResult<()>>;
}

/// Environment access for auth resolution. Injectable for tests and browsers.
pub trait AuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>>;
	/// Check whether a file exists. Supports a leading `~`. Always false in browsers.
	fn file_exists(&self, path: &str) -> LocalBoxFuture<'static, bool>;
}

/// Result of resolving auth for a model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuthResult {
	pub auth: ModelAuth,
	/// Provider-scoped environment/config values resolved from credentials and ambient context.
	pub env: Option<ProviderEnv>,
	/// Human-readable label for status UI: "ANTHROPIC_API_KEY", "OAuth", "~/.aws/credentials".
	pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuthCheck {
	pub source: Option<String>,
	pub r#type: AuthType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthType {
	#[serde(rename = "api_key")]
	ApiKey,
	#[serde(rename = "oauth")]
	OAuth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthSelectOption {
	pub id: String,
	pub label: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
}

/// Prompt shown to the user during login.
#[derive(Clone, Debug)]
pub enum AuthPrompt {
	Text {
		message: String,
		placeholder: Option<String>,
		signal: Option<AbortSignal>,
	},
	Secret {
		message: String,
		placeholder: Option<String>,
		signal: Option<AbortSignal>,
	},
	Select {
		message: String,
		options: Vec<AuthSelectOption>,
		signal: Option<AbortSignal>,
	},
	ManualCode {
		message: String,
		placeholder: Option<String>,
		signal: Option<AbortSignal>,
	},
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthInfoLink {
	pub url: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum AuthEvent {
	#[serde(rename = "info")]
	Info {
		message: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		links: Option<Vec<AuthInfoLink>>,
	},
	#[serde(rename = "auth_url")]
	AuthUrl {
		url: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		instructions: Option<String>,
	},
	#[serde(rename = "device_code")]
	DeviceCode {
		user_code: String,
		verification_uri: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		open_browser_on_confirm: Option<bool>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		copy_code_on_confirm: Option<bool>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		interval_seconds: Option<f64>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		expires_in_seconds: Option<f64>,
	},
	#[serde(rename = "progress")]
	Progress { message: String },
}

pub type AuthPromptFunction = Rc<dyn Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>>>;
pub type AuthNotifyFunction = Rc<dyn Fn(AuthEvent)>;

/// Login interaction callbacks serving both api-key and OAuth flows.
#[derive(Clone)]
pub struct AuthInteraction {
	pub signal: Option<AbortSignal>,
	pub prompt: AuthPromptFunction,
	pub notify: AuthNotifyFunction,
}

/// Normalized interaction passed to provider login implementations.
#[derive(Clone)]
pub struct ProviderAuthInteraction {
	pub signal: AbortSignal,
	pub prompt: AuthPromptFunction,
	pub notify: AuthNotifyFunction,
}

/// The input of `ApiKeyAuth.check` and `ApiKeyAuth.resolve`.
#[derive(Clone)]
pub struct ApiKeyAuthInput {
	pub ctx: Rc<dyn AuthContext>,
	pub credential: Option<ApiKeyCredential>,
	pub signal: AbortSignal,
}

pub type ApiKeyLoginFunction =
	Rc<dyn Fn(ProviderAuthInteraction) -> LocalBoxFuture<'static, PiResult<ApiKeyCredential>>>;
pub type ApiKeyCheckFunction = Rc<dyn Fn(ApiKeyAuthInput) -> LocalBoxFuture<'static, PiResult<Option<AuthCheck>>>>;
pub type ApiKeyResolveFunction = Rc<dyn Fn(ApiKeyAuthInput) -> LocalBoxFuture<'static, PiResult<Option<AuthResult>>>>;

/// Api-key auth: stored key/provider env plus ambient sources (env vars, AWS
/// profiles, ADC files). Ambient-only providers omit `login`.
#[derive(Clone)]
pub struct ApiKeyAuth {
	/// Display name, e.g. "Anthropic API key".
	pub name: String,
	/// Interactive setup (prompt for key/provider env). Absent = ambient-only.
	pub login: Option<ApiKeyLoginFunction>,
	/// Optional side-effect-free availability check.
	pub check: Option<ApiKeyCheckFunction>,
	/// Resolve auth from the stored credential and/or ambient sources.
	pub resolve: ApiKeyResolveFunction,
}

/// App-supplied context for `Models.login`.
#[derive(Clone, Default)]
pub struct LoginOptions {
	pub get_device_id: Option<Rc<dyn Fn() -> String>>,
	pub agent_name: Option<String>,
}

pub type OAuthLoginFunction =
	Rc<dyn Fn(ProviderAuthInteraction, Option<LoginOptions>) -> LocalBoxFuture<'static, PiResult<OAuthCredential>>>;
pub type OAuthRefreshFunction =
	Rc<dyn Fn(OAuthCredential, AbortSignal) -> LocalBoxFuture<'static, PiResult<OAuthCredential>>>;
pub type OAuthToAuthFunction = Rc<dyn Fn(OAuthCredential) -> LocalBoxFuture<'static, PiResult<ModelAuth>>>;

/// OAuth auth.
#[derive(Clone)]
pub struct OAuthAuth {
	pub name: String,
	pub is_subscription: Option<bool>,
	pub login_label: Option<String>,
	pub login: OAuthLoginFunction,
	pub refresh: OAuthRefreshFunction,
	pub to_auth: OAuthToAuthFunction,
}

/// Provider auth. At least one of `apiKey`/`oauth` must be present.
#[derive(Clone, Default)]
pub struct ProviderAuth {
	pub api_key: Option<ApiKeyAuth>,
	pub oauth: Option<OAuthAuth>,
}
