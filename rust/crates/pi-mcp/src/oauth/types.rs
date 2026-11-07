//! Port of `packages/mcp/src/oauth/types.ts`.
//! PORT-STATUS: reviewed
//!
//! Adapted from modelcontextprotocol/typescript-sdk v1.29.0. Copyright (c) 2024 Anthropic, PBC. Licensed under
//! MIT; see LICENSES/. Modified to use dependency-free structural validation.
//!
//! Metadata types with a TS index signature keep the other fields in a flattened `extra` map. The parsers
//! return the TS object's fields, but in declaration order followed by `extra` instead of the server's key
//! order. `OAuthClientInformationMixed` is an untagged enum: `Full` when `redirect_uris` is present.
//! `OAuthChallenge.resourceMetadataUrl` is a `Url`. `Number(expires_in)` parses strings with Rust's float
//! syntax, so hexadecimal strings such as `"0x10"` are rejected instead of read as numbers.

use pi_host::{PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthProtectedResourceMetadata {
	pub resource: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub authorization_servers: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scopes_supported: Option<Vec<String>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationServerMetadata {
	pub issuer: String,
	pub authorization_endpoint: String,
	pub token_endpoint: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub registration_endpoint: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scopes_supported: Option<Vec<String>>,
	pub response_types_supported: Vec<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub grant_types_supported: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub code_challenge_methods_supported: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub client_id_metadata_document_supported: Option<bool>,
	/// Whether authorization responses carry an `iss` parameter (RFC 9207).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub authorization_response_iss_parameter_supported: Option<bool>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthTokens {
	pub access_token: String,
	pub token_type: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub expires_in: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scope: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub refresh_token: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub id_token: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthClientMetadata {
	pub redirect_uris: Vec<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub token_endpoint_auth_method: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub grant_types: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub response_types: Option<Vec<String>>,
	/// OpenID Connect client type, `native` or `web`. Dynamic client registration derives it from
	/// `redirect_uris` when absent (MCP SEP-837).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub application_type: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub client_name: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub client_uri: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub logo_uri: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scope: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub contacts: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tos_uri: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub policy_uri: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub jwks_uri: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub jwks: Option<Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub software_id: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub software_version: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub software_statement: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthClientInformation {
	pub client_id: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub client_secret: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub client_id_issued_at: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub client_secret_expires_at: Option<f64>,
}

/// `OAuthClientInformation & OAuthClientMetadata`, plus the other fields of a registration response.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthClientInformationFull {
	#[serde(flatten)]
	pub information: OAuthClientInformation,
	#[serde(flatten)]
	pub metadata: OAuthClientMetadata,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

/// `OAuthClientInformation | OAuthClientInformationFull`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OAuthClientInformationMixed {
	Full(OAuthClientInformationFull),
	Information(OAuthClientInformation),
}

impl OAuthClientInformationMixed {
	/// The `OAuthClientInformation` fields.
	pub fn information(&self) -> &OAuthClientInformation {
		match self {
			Self::Full(full) => &full.information,
			Self::Information(information) => information,
		}
	}

	/// `"token_endpoint_auth_method" in information ? information.token_endpoint_auth_method : undefined`.
	pub fn token_endpoint_auth_method(&self) -> Option<&str> {
		match self {
			Self::Full(full) => full.metadata.token_endpoint_auth_method.as_deref(),
			Self::Information(_) => None,
		}
	}
}

impl From<OAuthClientInformation> for OAuthClientInformationMixed {
	fn from(information: OAuthClientInformation) -> Self {
		Self::Information(information)
	}
}

impl From<OAuthClientInformationFull> for OAuthClientInformationMixed {
	fn from(full: OAuthClientInformationFull) -> Self {
		Self::Full(full)
	}
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthDiscoveryState {
	pub authorization_server_url: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub authorization_server_metadata: Option<AuthorizationServerMetadata>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resource_metadata: Option<OAuthProtectedResourceMetadata>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resource_metadata_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthServerInfo {
	pub authorization_server_url: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub authorization_server_metadata: Option<AuthorizationServerMetadata>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resource_metadata: Option<OAuthProtectedResourceMetadata>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OAuthChallenge {
	pub resource_metadata_url: Option<Url>,
	pub scope: Option<String>,
	pub error: Option<String>,
	pub error_description: Option<String>,
}

fn invalid(name: &str) -> PiError {
	PiError::new(format!("Invalid {name}"))
}

fn object(value: Value, name: &str) -> PiResult<Map<String, Value>> {
	match value {
		Value::Object(object) => Ok(object),
		_ => Err(invalid(name)),
	}
}

fn required_string(value: Option<&Value>, name: &str) -> PiResult<String> {
	match value {
		Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
		_ => Err(invalid(name)),
	}
}

/// Treats `null` and `""` as absent: servers send them for fields they have no value for, like `scope: ""`.
fn absent(value: Option<&Value>) -> bool {
	match value {
		None | Some(Value::Null) => true,
		Some(Value::String(text)) => text.is_empty(),
		Some(_) => false,
	}
}

fn optional_string(value: Option<&Value>, name: &str) -> PiResult<Option<String>> {
	if absent(value) {
		return Ok(None);
	}
	required_string(value, name).map(Some)
}

fn optional_strings(value: Option<&Value>, name: &str) -> PiResult<Option<Vec<String>>> {
	match value {
		None | Some(Value::Null) => Ok(None),
		Some(Value::Array(items)) => items
			.iter()
			.map(|item| item.as_str().map(str::to_string).ok_or_else(|| invalid(name)))
			.collect::<PiResult<Vec<String>>>()
			.map(Some),
		Some(_) => Err(invalid(name)),
	}
}

/// Accepts only `http:` and `https:` URLs. Servers supply these URLs, and the authorization endpoint is
/// opened with the platform URL handler, so a `file:` or custom protocol URL would launch local programs.
fn safe_url(value: Option<&Value>, name: &str) -> PiResult<String> {
	let text = required_string(value, name)?;
	// URL parsing fails with a `TypeError` in TS, which discovery reserves for network failures.
	match Url::parse(&text) {
		Ok(url) if is_http_url(&url) => Ok(text),
		_ => Err(invalid(name)),
	}
}

/// `url.protocol === "http:" || url.protocol === "https:"`.
pub fn is_http_url(url: &Url) -> bool {
	url.scheme() == "http" || url.scheme() == "https"
}

fn optional_url(value: Option<&Value>, name: &str) -> PiResult<Option<String>> {
	if absent(value) {
		return Ok(None);
	}
	safe_url(value, name).map(Some)
}

fn optional_bool(value: Option<&Value>) -> Option<bool> {
	value.and_then(Value::as_bool)
}

fn optional_number(value: Option<&Value>) -> Option<f64> {
	value.filter(|value| value.is_number()).and_then(Value::as_f64)
}

/// The fields of `input` the typed struct does not hold.
fn extra(mut input: Map<String, Value>, known: &[&str]) -> Map<String, Value> {
	for key in known {
		input.shift_remove(*key);
	}
	input
}

/// `parseProtectedResourceMetadata(value)`.
pub fn parse_protected_resource_metadata(value: Value) -> PiResult<OAuthProtectedResourceMetadata> {
	let input = object(value, "OAuth protected resource metadata")?;
	let resource = safe_url(input.get("resource"), "OAuth protected resource metadata resource")?;
	let authorization_servers = optional_strings(input.get("authorization_servers"), "authorization_servers")?
		.map(|urls| {
			urls.into_iter()
				.map(|url| safe_url(Some(&Value::String(url)), "authorization server URL"))
				.collect::<PiResult<Vec<String>>>()
		})
		.transpose()?;
	let scopes_supported = optional_strings(input.get("scopes_supported"), "scopes_supported")?;
	Ok(OAuthProtectedResourceMetadata {
		resource,
		authorization_servers,
		scopes_supported,
		extra: extra(input, &["resource", "authorization_servers", "scopes_supported"]),
	})
}

/// `parseAuthorizationServerMetadata(value)`.
pub fn parse_authorization_server_metadata(value: Value) -> PiResult<AuthorizationServerMetadata> {
	let input = object(value, "authorization server metadata")?;
	let Some(response_types_supported) =
		optional_strings(input.get("response_types_supported"), "response_types_supported")?
	else {
		return Err(invalid("response_types_supported"));
	};
	let metadata = AuthorizationServerMetadata {
		issuer: safe_url(input.get("issuer"), "authorization server issuer")?,
		authorization_endpoint: safe_url(input.get("authorization_endpoint"), "authorization endpoint")?,
		token_endpoint: safe_url(input.get("token_endpoint"), "token endpoint")?,
		registration_endpoint: optional_url(input.get("registration_endpoint"), "registration endpoint")?,
		scopes_supported: optional_strings(input.get("scopes_supported"), "scopes_supported")?,
		response_types_supported,
		grant_types_supported: optional_strings(input.get("grant_types_supported"), "grant_types_supported")?,
		token_endpoint_auth_methods_supported: optional_strings(
			input.get("token_endpoint_auth_methods_supported"),
			"token_endpoint_auth_methods_supported",
		)?,
		code_challenge_methods_supported: optional_strings(
			input.get("code_challenge_methods_supported"),
			"code_challenge_methods_supported",
		)?,
		client_id_metadata_document_supported: optional_bool(input.get("client_id_metadata_document_supported")),
		authorization_response_iss_parameter_supported: optional_bool(
			input.get("authorization_response_iss_parameter_supported"),
		),
		extra: Map::new(),
	};
	Ok(AuthorizationServerMetadata {
		extra: extra(
			input,
			&[
				"issuer",
				"authorization_endpoint",
				"token_endpoint",
				"registration_endpoint",
				"scopes_supported",
				"response_types_supported",
				"grant_types_supported",
				"token_endpoint_auth_methods_supported",
				"code_challenge_methods_supported",
				"client_id_metadata_document_supported",
				"authorization_response_iss_parameter_supported",
			],
		),
		..metadata
	})
}

/// `Number(value)` for the JSON values a token response may carry.
fn js_number(value: &Value) -> f64 {
	match value {
		Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
		Value::Bool(flag) => f64::from(u8::from(*flag)),
		Value::String(text) => {
			let text = text.trim();
			if text.is_empty() {
				0.0
			} else {
				text.parse().unwrap_or(f64::NAN)
			}
		}
		_ => f64::NAN,
	}
}

/// `parseOAuthTokens(value)`.
pub fn parse_oauth_tokens(value: Value) -> PiResult<OAuthTokens> {
	let input = object(value, "OAuth token response")?;
	// `Number(null)` is 0, which would mark the token as expired at once.
	let expires_in = if absent(input.get("expires_in")) {
		None
	} else {
		Some(js_number(&input["expires_in"]))
	};
	if expires_in.is_some_and(|expires| !expires.is_finite()) {
		return Err(invalid("expires_in"));
	}
	Ok(OAuthTokens {
		access_token: required_string(input.get("access_token"), "access_token")?,
		token_type: required_string(input.get("token_type"), "token_type")?,
		expires_in,
		scope: optional_string(input.get("scope"), "scope")?,
		refresh_token: optional_string(input.get("refresh_token"), "refresh_token")?,
		id_token: optional_string(input.get("id_token"), "id_token")?,
	})
}

/// `parseClientInformation(value)`. The client metadata fields are read leniently, as the TS cast does:
/// a field of the wrong type is dropped instead of failing the registration.
pub fn parse_client_information(value: Value) -> PiResult<OAuthClientInformationFull> {
	let input = object(value, "OAuth client registration response")?;
	let information = OAuthClientInformation {
		client_id: required_string(input.get("client_id"), "client_id")?,
		client_secret: optional_string(input.get("client_secret"), "client_secret")?,
		client_id_issued_at: optional_number(input.get("client_id_issued_at")),
		client_secret_expires_at: optional_number(input.get("client_secret_expires_at")),
	};
	let redirect_uris = optional_strings(input.get("redirect_uris"), "redirect_uris")?.unwrap_or_default();
	let mut rest = extra(
		input,
		&[
			"client_id",
			"client_secret",
			"client_id_issued_at",
			"client_secret_expires_at",
			"redirect_uris",
		],
	);
	let mut take = |key: &str| rest.shift_remove(key);
	let string = |value: Option<Value>| value.and_then(|value| value.as_str().map(str::to_string));
	let strings = |value: Option<Value>| value.and_then(|value| serde_json::from_value::<Vec<String>>(value).ok());
	let metadata = OAuthClientMetadata {
		redirect_uris,
		token_endpoint_auth_method: string(take("token_endpoint_auth_method")),
		grant_types: strings(take("grant_types")),
		response_types: strings(take("response_types")),
		application_type: string(take("application_type")),
		client_name: string(take("client_name")),
		client_uri: string(take("client_uri")),
		logo_uri: string(take("logo_uri")),
		scope: string(take("scope")),
		contacts: strings(take("contacts")),
		tos_uri: string(take("tos_uri")),
		policy_uri: string(take("policy_uri")),
		jwks_uri: string(take("jwks_uri")),
		jwks: take("jwks"),
		software_id: string(take("software_id")),
		software_version: string(take("software_version")),
		software_statement: string(take("software_statement")),
	};
	Ok(OAuthClientInformationFull {
		information,
		metadata,
		extra: rest,
	})
}
