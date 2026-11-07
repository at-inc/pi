//! Port of `packages/ai/src/auth/types.ts`.
//! PORT-STATUS: reviewed
//!
//! The TS interfaces that apps implement and that carry state (`CredentialStore`, `AuthContext`) are traits used as
//! `Rc<dyn ...>`. The interfaces that are plain objects of callbacks (`AuthInteraction`, `ApiKeyAuth`, `OAuthAuth`)
//! are structs of `Rc` closures with methods of the same name that call them, so provider code builds them like
//! the TS object literals. `AuthPrompt`'s anonymous union is `AuthPromptKind`, its option type is
//! `AuthSelectOption`, and the input of `ApiKeyAuth.check`/`resolve` is `ApiKeyAuthInput`. `OAuthCredential`
//! (`OAuthCredentials & { type: "oauth" }`) repeats the `OAuthCredentials` fields and writes its `type` tag first,
//! then `refresh`, `access`, `expires` in declaration order, then provider fields in insertion order; a TS object
//! literal that listed `access` before `refresh` stringifies with the other order.
//! Deserialization accepts what the TS runtime keeps: a non-string `refresh`, `access`, `key` or env value becomes its
//! `String(value)` (a missing or `null` token becomes `""`), and `expires` is `Number(value)` (missing is `NaN`,
//! `null` is `0`). Unknown keys of both credential types survive in `extra`. A credential whose `type` is neither
//! `api_key` nor `oauth` does not deserialize, where the TS store keeps it and resolution ignores it.

use std::ops::Deref;
use std::rc::Rc;

use pi_host::{AbortSignal, LocalBoxFuture, PiResult};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::types::{ProviderEnv, ProviderHeaders};

/// Request auth for a single model request. If a value cannot be expressed as
/// `apiKey`, `headers`, or `baseUrl`, it is provider config, not auth.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAuth {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub api_key: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub headers: Option<ProviderHeaders>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub base_url: Option<String>,
}

/// Stored api-key credential. `env` holds provider-scoped environment/config
/// values such as Cloudflare account/gateway ids.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApiKeyCredential {
	pub key: Option<String>,
	pub env: Option<ProviderEnv>,
	/// App fields of the stored object, in insertion order.
	pub extra: Map<String, Value>,
}

impl Serialize for ApiKeyCredential {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		let mut object = Map::new();
		object.insert("type".into(), Value::from("api_key"));
		if let Some(key) = &self.key {
			object.insert("key".into(), Value::from(key.as_str()));
		}
		if let Some(env) = &self.env {
			object.insert(
				"env".into(),
				serde_json::to_value(env).map_err(serde::ser::Error::custom)?,
			);
		}
		for (key, value) in &self.extra {
			object.insert(key.clone(), value.clone());
		}
		object.serialize(serializer)
	}
}

impl<'de> Deserialize<'de> for ApiKeyCredential {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let mut object = Map::deserialize(deserializer)?;
		object.shift_remove("type");
		let key = object.shift_remove("key").and_then(|key| optional_js_string(&key));
		let env = match object.shift_remove("env") {
			Some(Value::Object(env)) => Some(
				env.iter()
					.filter_map(|(name, value)| optional_js_string(value).map(|value| (name.clone(), value)))
					.collect(),
			),
			_ => None,
		};
		Ok(Self {
			key,
			env,
			extra: object,
		})
	}
}

/// OAuth token data returned by extension compatibility flows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthCredentials {
	pub refresh: String,
	pub access: String,
	pub expires: f64,
	/// `[key: string]: unknown`: provider fields such as `accountId` or `enterpriseUrl`, in insertion order.
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

/// Stored canonical OAuth credential.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(tag = "type", rename = "oauth")]
pub struct OAuthCredential {
	pub refresh: String,
	pub access: String,
	pub expires: f64,
	/// Provider fields such as `accountId` or `enterpriseUrl`, in insertion order.
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

impl OAuthCredential {
	/// `{ type: "oauth", refresh, access, expires }`.
	pub fn new(refresh: impl Into<String>, access: impl Into<String>, expires: f64) -> Self {
		Self {
			refresh: refresh.into(),
			access: access.into(),
			expires,
			extra: Map::new(),
		}
	}

	/// `credential[key]` for a provider field.
	pub fn field(&self, key: &str) -> Option<&Value> {
		self.extra.get(key)
	}

	/// `credential[key] = value`; an existing key keeps its position, like a JS property assignment.
	pub fn set_field(&mut self, key: &str, value: Value) {
		match key {
			"refresh" => self.refresh = token_string(Some(value)),
			"access" => self.access = token_string(Some(value)),
			"expires" => self.expires = js_number(Some(value)),
			_ => {
				self.extra.insert(key.into(), value);
			}
		}
	}
}

impl<'de> Deserialize<'de> for OAuthCredential {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let mut object = Map::deserialize(deserializer)?;
		object.shift_remove("type");
		let refresh = token_string(object.shift_remove("refresh"));
		let access = token_string(object.shift_remove("access"));
		let expires = js_number(object.shift_remove("expires"));
		Ok(Self {
			refresh,
			access,
			expires,
			extra: object,
		})
	}
}

/// JS `String(value)` of a stored value; `null` is absent.
fn optional_js_string(value: &Value) -> Option<String> {
	match value {
		Value::Null => None,
		Value::String(value) => Some(value.clone()),
		Value::Number(number) => Some(pi_host::js::number_to_string(number.as_f64().unwrap_or(f64::NAN))),
		Value::Bool(value) => Some(value.to_string()),
		Value::Array(_) | Value::Object(_) => Some(pi_host::js::json_stringify(value)),
	}
}

/// A stored token: its `String(value)`, or `""` when missing or `null`.
fn token_string(value: Option<Value>) -> String {
	value.as_ref().and_then(optional_js_string).unwrap_or_default()
}

/// JS `Number(value)` of a stored field; a missing field is `NaN`.
fn js_number(value: Option<Value>) -> f64 {
	match value {
		None => f64::NAN,
		Some(Value::Null) => 0.0,
		Some(Value::Bool(value)) => f64::from(u8::from(value)),
		Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
		Some(Value::String(value)) => pi_host::js::string_to_number(&value),
		Some(_) => f64::NAN,
	}
}

/// One type-tagged credential per provider — the shape of today's auth.json.
#[derive(Clone, Debug, PartialEq)]
pub enum Credential {
	ApiKey(ApiKeyCredential),
	OAuth(OAuthCredential),
}

impl Credential {
	/// `credential.type`.
	pub fn r#type(&self) -> AuthType {
		match self {
			Self::ApiKey(_) => AuthType::ApiKey,
			Self::OAuth(_) => AuthType::OAuth,
		}
	}
}

impl Serialize for Credential {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			Self::ApiKey(credential) => credential.serialize(serializer),
			Self::OAuth(credential) => credential.serialize(serializer),
		}
	}
}

impl<'de> Deserialize<'de> for Credential {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = Value::deserialize(deserializer)?;
		match value.get("type").and_then(Value::as_str) {
			Some("api_key") => ApiKeyCredential::deserialize(value)
				.map(Self::ApiKey)
				.map_err(serde::de::Error::custom),
			Some("oauth") => OAuthCredential::deserialize(value)
				.map(Self::OAuth)
				.map_err(serde::de::Error::custom),
			_ => Err(serde::de::Error::custom("unknown credential type")),
		}
	}
}

/// Non-secret credential metadata for account/status enumeration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// `fn` of `CredentialStore.modify`: sees the current credential and returns the new one, or `None` to leave the
/// entry unchanged.
pub type CredentialModifier =
	Box<dyn FnOnce(Option<Credential>) -> LocalBoxFuture<'static, PiResult<Option<Credential>>>>;

/// App-owned credential storage, keyed by `Provider.id`, one credential per
/// provider. `modify` is the only write path, so every mutation is a
/// serialized read-modify-write; `Models.getAuth()` runs OAuth refresh inside
/// `modify` so concurrent requests cannot double-refresh a rotated token. The
/// app persists a credential after login via
/// `modify(provider.id, async () => credential)`. Login/logout orchestration
/// is app-owned.
///
/// Error semantics: `read` resolves `undefined` for missing entries. Methods
/// reject only on storage failure; `Models` wraps such rejections in
/// `ModelsError` with code "auth". Best-effort stores that serve an in-memory
/// view and record persistence errors internally (like coding-agent's
/// AuthStorage) are valid implementations.
pub trait CredentialStore {
	/// Read the stored credential, possibly expired. Display/status use;
	/// resolved request auth comes from `Models.getAuth()`.
	fn read(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>>;

	/// List stored credential metadata without resolving or exposing secrets.
	/// Implementations must not execute configured API-key commands while listing.
	fn list(&self, options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>>;

	/// Serialized write — the only write path. `fn` sees the current credential
	/// because correct writes (refresh, login-during-refresh) depend on it;
	/// return the new credential, or undefined to leave the entry unchanged.
	/// Mutual exclusion per provider id, cross-process too where the backing
	/// store supports it (e.g. a file lock). Resolves with the post-write
	/// credential. Rejections from `fn` propagate.
	fn modify(
		&self,
		provider_id: &str,
		r#fn: CredentialModifier,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>>;

	/// Remove a credential (logout). Implementations serialize this against `modify`.
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResult {
	pub auth: ModelAuth,
	/// Provider-scoped environment/config values resolved from credentials and ambient context.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub env: Option<ProviderEnv>,
	/// Human-readable label for status UI: "ANTHROPIC_API_KEY", "OAuth", "~/.aws/credentials".
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthCheck {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub source: Option<String>,
	#[serde(rename = "type")]
	pub r#type: AuthType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthType {
	#[serde(rename = "api_key")]
	ApiKey,
	#[serde(rename = "oauth")]
	OAuth,
}

/// Prompt shown to the user during login. `signal` lets the flow cancel a
/// pending prompt when an out-of-band event resolves the step, e.g. a
/// `manual_code` prompt raced against a callback server, aborted when the
/// callback wins.
#[derive(Clone, Debug)]
pub struct AuthPrompt {
	pub signal: Option<AbortSignal>,
	pub kind: AuthPromptKind,
}

impl AuthPrompt {
	/// A prompt without its own signal.
	pub fn new(kind: AuthPromptKind) -> Self {
		Self { signal: None, kind }
	}
}

impl Deref for AuthPrompt {
	type Target = AuthPromptKind;

	fn deref(&self) -> &Self::Target {
		&self.kind
	}
}

/// The prompt variants of `AuthPrompt`, tagged by `type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthPromptKind {
	Text {
		message: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		placeholder: Option<String>,
	},
	Secret {
		message: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		placeholder: Option<String>,
	},
	Select {
		message: String,
		options: Vec<AuthSelectOption>,
	},
	ManualCode {
		message: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		placeholder: Option<String>,
	},
}

impl AuthPromptKind {
	/// `prompt.message`.
	pub fn message(&self) -> &str {
		match self {
			Self::Text { message, .. }
			| Self::Secret { message, .. }
			| Self::Select { message, .. }
			| Self::ManualCode { message, .. } => message,
		}
	}

	/// `prompt.placeholder`; `select` prompts have none.
	pub fn placeholder(&self) -> Option<&str> {
		match self {
			Self::Text { placeholder, .. }
			| Self::Secret { placeholder, .. }
			| Self::ManualCode { placeholder, .. } => placeholder.as_deref(),
			Self::Select { .. } => None,
		}
	}

	/// `prompt.type`.
	pub fn type_name(&self) -> &'static str {
		match self {
			Self::Text { .. } => "text",
			Self::Secret { .. } => "secret",
			Self::Select { .. } => "select",
			Self::ManualCode { .. } => "manual_code",
		}
	}
}

/// An option of a `select` prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSelectOption {
	pub id: String,
	pub label: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
}

impl AuthSelectOption {
	/// `{ id, label }`.
	pub fn new(id: &str, label: &str) -> Self {
		Self {
			id: id.into(),
			label: label.into(),
			description: None,
		}
	}
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthInfoLink {
	pub url: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AuthEvent {
	Info {
		message: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		links: Option<Vec<AuthInfoLink>>,
	},
	AuthUrl {
		url: String,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		instructions: Option<String>,
	},
	DeviceCode {
		user_code: String,
		verification_uri: String,
		/// Offer best-effort browser opening on user confirmation. Defaults to true; clients may ignore it.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		open_browser_on_confirm: Option<bool>,
		/// Offer copying of userCode on user confirmation, replacing the clipboard. Defaults to true.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		copy_code_on_confirm: Option<bool>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		interval_seconds: Option<f64>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		expires_in_seconds: Option<f64>,
	},
	Progress {
		message: String,
	},
}

/// `AuthInteraction.prompt`.
pub type AuthPromptFunction = Rc<dyn Fn(AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>>>;
/// `AuthInteraction.notify`.
pub type AuthNotifyFunction = Rc<dyn Fn(AuthEvent)>;

/// Login interaction callbacks serving both api-key and OAuth flows.
///
/// `prompt()` returns the entered/selected string (`select` returns the option
/// id). Rejects on cancel/abort. `signal` aborts the whole login flow;
/// per-prompt cancellation uses `AuthPrompt.signal`.
#[derive(Clone)]
pub struct AuthInteraction {
	pub signal: Option<AbortSignal>,
	pub prompt: AuthPromptFunction,
	pub notify: AuthNotifyFunction,
}

impl AuthInteraction {
	/// `interaction.prompt(prompt)`.
	pub fn prompt(&self, prompt: AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> {
		(self.prompt)(prompt)
	}

	/// `interaction.notify(event)`.
	pub fn notify(&self, event: AuthEvent) {
		(self.notify)(event)
	}
}

/// Normalized interaction passed to provider login implementations.
#[derive(Clone)]
pub struct ProviderAuthInteraction {
	pub signal: AbortSignal,
	pub prompt: AuthPromptFunction,
	pub notify: AuthNotifyFunction,
}

impl ProviderAuthInteraction {
	/// `interaction.prompt(prompt)`.
	pub fn prompt(&self, prompt: AuthPrompt) -> LocalBoxFuture<'static, PiResult<String>> {
		(self.prompt)(prompt)
	}

	/// `interaction.notify(event)`.
	pub fn notify(&self, event: AuthEvent) {
		(self.notify)(event)
	}
}

/// The input of `ApiKeyAuth.check` and `ApiKeyAuth.resolve`.
#[derive(Clone)]
pub struct ApiKeyAuthInput {
	pub ctx: Rc<dyn AuthContext>,
	pub credential: Option<ApiKeyCredential>,
	pub signal: AbortSignal,
}

/// `ApiKeyAuth.login`.
pub type ApiKeyLoginFunction =
	Rc<dyn Fn(ProviderAuthInteraction) -> LocalBoxFuture<'static, PiResult<ApiKeyCredential>>>;
/// `ApiKeyAuth.check`.
pub type ApiKeyCheckFunction = Rc<dyn Fn(ApiKeyAuthInput) -> LocalBoxFuture<'static, PiResult<Option<AuthCheck>>>>;
/// `ApiKeyAuth.resolve`.
pub type ApiKeyResolveFunction = Rc<dyn Fn(ApiKeyAuthInput) -> LocalBoxFuture<'static, PiResult<Option<AuthResult>>>>;

/// Api-key auth: stored key/provider env plus ambient sources (env vars, AWS
/// profiles, ADC files). Ambient-only providers omit `login`.
#[derive(Clone)]
pub struct ApiKeyAuth {
	/// Display name, e.g. "Anthropic API key".
	pub name: String,

	/// Interactive setup (prompt for key/provider env). Absent = ambient-only.
	pub login: Option<ApiKeyLoginFunction>,

	/// Optional side-effect-free availability check. Use this when `resolve()` may
	/// execute commands or perform other request-time work. Missing means Models
	/// checks availability by resolving auth.
	pub check: Option<ApiKeyCheckFunction>,

	/// Resolve auth from the stored credential and/or ambient sources, merging
	/// per field (`credential.key ?? env("...")`, `credential.env?.NAME ?? env("...")`).
	/// undefined = not configured. Resolution is provider-scoped; model-specific
	/// endpoint preparation happens after auth has been resolved.
	pub resolve: ApiKeyResolveFunction,
}

impl ApiKeyAuth {
	/// `apiKey.resolve(input)`.
	pub fn resolve(&self, input: ApiKeyAuthInput) -> LocalBoxFuture<'static, PiResult<Option<AuthResult>>> {
		(self.resolve)(input)
	}
}

/// `LoginOptions.getDeviceId`.
pub type GetDeviceIdFunction = Rc<dyn Fn() -> String>;

/// App-supplied context for `Models.login`.
#[derive(Clone, Default)]
pub struct LoginOptions {
	/// Returns the stable ID of this app installation, e.g. sent to OpenAI as its
	/// agent host ID. Called only by login flows that need it, so apps can create
	/// the ID on first use and must return the same ID on every later call.
	pub get_device_id: Option<GetDeviceIdFunction>,
	/// Name this app introduces itself with during login, e.g. OpenAI's agent name hint and
	/// Codex originator. Defaults to pi's own name.
	pub agent_name: Option<String>,
}

/// `OAuthAuth.login`.
pub type OAuthLoginFunction =
	Rc<dyn Fn(ProviderAuthInteraction, Option<LoginOptions>) -> LocalBoxFuture<'static, PiResult<OAuthCredential>>>;
/// `OAuthAuth.refresh`.
pub type OAuthRefreshFunction =
	Rc<dyn Fn(OAuthCredential, AbortSignal) -> LocalBoxFuture<'static, PiResult<OAuthCredential>>>;
/// `OAuthAuth.toAuth`.
pub type OAuthToAuthFunction = Rc<dyn Fn(OAuthCredential) -> LocalBoxFuture<'static, PiResult<ModelAuth>>>;

/// OAuth auth. The `refresh`/`toAuth` split lets `Models` own the locked
/// refresh pattern: `refresh` produces a credential, `toAuth` derives request
/// auth from whatever credential ends up stored.
#[derive(Clone)]
pub struct OAuthAuth {
	/// Display name, e.g. "Anthropic (Claude Pro/Max)".
	pub name: String,

	/// Whether access through this auth method is backed by a provider subscription.
	pub is_subscription: Option<bool>,

	/// Selector label for the OAuth login option, e.g. "Sign in with SuperGrok or X Premium".
	pub login_label: Option<String>,

	pub login: OAuthLoginFunction,

	/// Exchange the refresh token. Network call; throws on failure
	/// (invalid_grant etc.). `Models` runs this under the store lock.
	pub refresh: OAuthRefreshFunction,

	/// Side-effect-free derivation of request auth from a valid credential.
	/// Covers per-credential baseUrl (GitHub Copilot). Async so lazy wrappers
	/// can load the implementation on first use.
	pub to_auth: OAuthToAuthFunction,
}

impl OAuthAuth {
	/// `oauth.login(interaction, options)`.
	pub fn login(
		&self,
		interaction: ProviderAuthInteraction,
		options: Option<LoginOptions>,
	) -> LocalBoxFuture<'static, PiResult<OAuthCredential>> {
		(self.login)(interaction, options)
	}

	/// `oauth.refresh(credential, signal)`.
	pub fn refresh(
		&self,
		credential: OAuthCredential,
		signal: AbortSignal,
	) -> LocalBoxFuture<'static, PiResult<OAuthCredential>> {
		(self.refresh)(credential, signal)
	}

	/// `oauth.toAuth(credential)`.
	pub fn to_auth(&self, credential: OAuthCredential) -> LocalBoxFuture<'static, PiResult<ModelAuth>> {
		(self.to_auth)(credential)
	}
}

/// Provider auth. At least one of `apiKey`/`oauth` must be present: even
/// ambient-credential providers and keyless local servers provide `apiKey`
/// auth whose `resolve()` reports whether the provider is configured.
#[derive(Clone, Default)]
pub struct ProviderAuth {
	pub api_key: Option<ApiKeyAuth>,
	pub oauth: Option<OAuthAuth>,
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	fn credential(value: Value) -> Credential {
		serde_json::from_value(value).unwrap()
	}

	#[test]
	fn deserializes_what_the_ts_runtime_keeps() {
		let Credential::OAuth(missing_expires) = credential(json!({ "type": "oauth", "access": "a", "refresh": "r" }))
		else {
			panic!("oauth");
		};
		assert!(missing_expires.expires.is_nan());
		let Credential::OAuth(loose) = credential(json!({ "type": "oauth", "access": null, "expires": "123" })) else {
			panic!("oauth");
		};
		assert_eq!(
			(loose.access.as_str(), loose.refresh.as_str(), loose.expires),
			("", "", 123.0)
		);
		let Credential::OAuth(null_expires) = credential(json!({ "type": "oauth", "access": "a", "expires": null }))
		else {
			panic!("oauth");
		};
		assert_eq!(null_expires.expires, 0.0);
		let Credential::ApiKey(api_key) =
			credential(json!({ "type": "api_key", "key": 5, "env": { "PORT": 8080 }, "command": "op read" }))
		else {
			panic!("api_key");
		};
		assert_eq!(api_key.key.as_deref(), Some("5"));
		assert_eq!(api_key.env.as_ref().unwrap()["PORT"], "8080");
		assert_eq!(
			serde_json::to_value(Credential::ApiKey(api_key)).unwrap(),
			json!({ "type": "api_key", "key": "5", "env": { "PORT": "8080" }, "command": "op read" })
		);
	}

	#[test]
	fn set_field_assigns_the_token_fields() {
		let mut credential = OAuthCredential::new("r", "a", 1.0);
		credential.set_field("access", json!("b"));
		credential.set_field("accountId", json!("x"));
		assert_eq!(credential.access, "b");
		assert_eq!(
			serde_json::to_value(&credential).unwrap(),
			json!({ "type": "oauth", "refresh": "r", "access": "b", "expires": 1.0, "accountId": "x" })
		);
	}
}
