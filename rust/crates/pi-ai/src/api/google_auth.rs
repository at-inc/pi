//! Rust-only replacement for the parts of `google-auth-library` (with `gtoken`, `gaxios` and `gcp-metadata`) that the
//! `@google/genai` Node client uses for Vertex AI Application Default Credentials: `GoogleAuth.getRequestHeaders` with
//! a `keyFilename` or the ADC search (`GOOGLE_APPLICATION_CREDENTIALS`, the gcloud well-known file, then the GCE
//! metadata server), service-account JWT bearer tokens (RS256 via `rsa`, or a self-signed JWT outside the default
//! universe), `authorized_user` refresh tokens, Compute Engine metadata tokens, `x-goog-user-project` from the quota
//! project (and `GOOGLE_CLOUD_QUOTA_PROJECT` for ADC credentials), token expiry with the 5-minute eager refresh, and
//! gaxios's retry policy for the token requests.
//! PORT-STATUS: reviewed
//!
//! Differences from the library:
//! - `impersonated_service_account`, `external_account` and `external_account_authorized_user` credential files fail
//!   with `Unsupported credential type: <type>`, also as a `keyFilename`.
//! - ADC does not look up the project id: the genai client never reads it, so the request headers are the same. The
//!   library runs `findAndCacheProjectId` on every ADC resolution before the token request: the project env vars, the
//!   credential's `project_id`, a `gcloud config config-helper` subprocess, then the metadata `project/project-id`
//!   (3 s timeout, up to 3 retries), so on a machine without a configured project it can wait seconds first.
//! - Token and metadata requests go through `pi_host::fetch`; gaxios uses `node-fetch`, so a stubbed
//!   `globalThis.fetch` never sees them in Node. A network failure reports the host's `fetch failed` without a
//!   `code`, where node-fetch reports `request to <url> failed, reason: <cause>` with the system error code.
//! - Concurrent first uses of one client are not de-duplicated (`#pendingAuthClient`, `refreshTokenPromises`,
//!   gtoken's `inFlightRequest`, the cached `isAvailable` promise): two racing calls can send two token or metadata
//!   requests. pi-ai builds one client per request.
//! - A credential file that is not valid JSON fails with serde_json's message instead of V8's `SyntaxError` text.
//! - The GCE residency probe reads the Linux BIOS vendor file and the serverless env vars; the network-interface MAC
//!   check has no host API and is skipped. Paths are passed to the host as given (no `path.resolve` / `realpath`), and a
//!   missing ADC file reports the `lstat` of the whole path.
//! - `gl-node/<version>` in `x-goog-api-client` is `gl-rust/<rust-version>`; an unreadable private key fails with the
//!   `rsa` crate's message instead of OpenSSL's.

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use indexmap::IndexMap;
use pi_host::js::{js_key_order, json_stringify, string_to_number};
use pi_host::{AbortSignal, HttpRequest, PiError, PiResult};
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use serde_json::{Map, Value, json};
use sha2::Sha256;

use super::google_client;
use super::google_client::js_truthy;
use crate::utils::text::js_trim;

/// `REQUIRED_VERTEX_AI_SCOPE` of the genai client.
pub const REQUIRED_VERTEX_AI_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const DEFAULT_UNIVERSE: &str = "googleapis.com";
const DEFAULT_EAGER_REFRESH_THRESHOLD_MILLIS: f64 = 5.0 * 60.0 * 1000.0;
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const NO_ADC_FOUND: &str = "Could not load the default credentials. Browse to https://cloud.google.com/docs/authentication/getting-started for more information.";
const METADATA_HOST_ADDRESS: &str = "http://169.254.169.254";
const METADATA_SECONDARY_HOST_ADDRESS: &str = "http://metadata.google.internal.";
const METADATA_BASE_PATH: &str = "/computeMetadata/v1";
const UNSUPPORTED_CREDENTIAL_TYPE: &str = "Unsupported credential type: ";
/// `USER_AGENT` of google-auth-library 10.6.2.
const AUTH_LIBRARY_USER_AGENT: &str = "google-api-nodejs-client/10.6.2";

/// `GoogleAuthOptions` (the fields the genai client passes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GoogleAuthOptions {
	pub key_filename: Option<String>,
	pub scopes: Option<Vec<String>>,
}

/// `new GoogleAuth(options)`; the auth client is resolved once and cached, like `getClient()`.
pub struct GoogleAuth {
	options: GoogleAuthOptions,
	client: RefCell<Option<Rc<AuthClient>>>,
}

impl GoogleAuth {
	/// `new GoogleAuth(options)`.
	pub fn new(options: GoogleAuthOptions) -> Self {
		Self {
			options,
			client: RefCell::new(None),
		}
	}

	/// `getRequestHeaders(url)`: the `authorization` (and `x-goog-user-project`) headers, in `Headers` order.
	pub async fn get_request_headers(&self, url: &str) -> PiResult<Vec<(String, String)>> {
		let client = self.get_client().await?;
		client.get_request_headers(url).await
	}

	async fn get_client(&self) -> PiResult<Rc<AuthClient>> {
		if let Some(client) = self.client.borrow().clone() {
			return Ok(client);
		}
		let client = Rc::new(self.determine_client().await?);
		*self.client.borrow_mut() = Some(client.clone());
		Ok(client)
	}

	async fn determine_client(&self) -> PiResult<AuthClient> {
		let scopes = self.options.scopes.clone().unwrap_or_default();
		if let Some(key_filename) = &self.options.key_filename {
			let content = pi_host::read_file(key_filename).await?;
			// `fromStreamAsync`: any parse or `_cacheClientFromJSON` failure falls back to `new JWT({ keyFile })`.
			return match serde_json::from_str::<Value>(&content).map(|json| from_json(&json, scopes)) {
				Ok(Ok(client)) => Ok(client),
				Ok(Err(error)) if error.message.starts_with(UNSUPPORTED_CREDENTIAL_TYPE) => Err(error),
				_ => Ok(AuthClient::new(Credential::UnsignedKeyFile, Vec::new(), None, None)),
			};
		}
		self.get_application_default(scopes).await
	}

	/// `getApplicationDefaultAsync`.
	async fn get_application_default(&self, scopes: Vec<String>) -> PiResult<AuthClient> {
		let credentials_path = pi_host::env("GOOGLE_APPLICATION_CREDENTIALS")
			.filter(|path| !path.is_empty())
			.or_else(|| pi_host::env("google_application_credentials"));
		if let Some(path) = credentials_path.filter(|path| !path.is_empty()) {
			let client = credentials_from_file_path(&path, scopes).await?;
			return Ok(with_quota_project_override(client));
		}
		if let Some(location) = well_known_file_location() {
			let client = credentials_from_file_path(&location, scopes).await?;
			return Ok(with_quota_project_override(client));
		}
		if check_is_gce().await? {
			return Ok(with_quota_project_override(AuthClient::new(
				Credential::Compute,
				scopes,
				None,
				None,
			)));
		}
		Err(PiError::new(NO_ADC_FOUND))
	}
}

/// `#prepareAndCacheClient`: `GOOGLE_CLOUD_QUOTA_PROJECT` overrides the credential's quota project.
fn with_quota_project_override(mut client: AuthClient) -> AuthClient {
	if let Some(quota_project) = pi_host::env("GOOGLE_CLOUD_QUOTA_PROJECT").filter(|value| !value.is_empty()) {
		client.quota_project_id = Some(quota_project);
	}
	client
}

/// `_tryGetApplicationCredentialsFromWellKnownFile`: the gcloud ADC file, when it exists.
fn well_known_file_location() -> Option<String> {
	let location = if cfg!(windows) {
		pi_host::env("APPDATA")?
	} else {
		let home = pi_host::env("HOME").filter(|home| !home.is_empty())?;
		std::path::Path::new(&home)
			.join(".config")
			.to_string_lossy()
			.into_owned()
	};
	if location.is_empty() {
		return None;
	}
	let location = std::path::Path::new(&location)
		.join("gcloud")
		.join("application_default_credentials.json")
		.to_string_lossy()
		.into_owned();
	pi_host::file_exists(&location).then_some(location)
}

/// `_getApplicationCredentialsFromFilePath(filePath)`.
async fn credentials_from_file_path(file_path: &str, scopes: Vec<String>) -> PiResult<AuthClient> {
	if !pi_host::file_exists(file_path) {
		return Err(PiError::new(format!(
			"The file at {file_path} does not exist, or it is not a file. ENOENT: no such file or directory, lstat '{file_path}'"
		)));
	}
	let content = pi_host::read_file(file_path).await?;
	let json = serde_json::from_str::<Value>(&content)?;
	from_json(&json, scopes)
}

/// `GoogleAuth.fromJSON(json)`.
fn from_json(json: &Value, scopes: Vec<String>) -> PiResult<AuthClient> {
	let field = |name: &str| json.get(name).and_then(Value::as_str).filter(|value| !value.is_empty());
	let universe_domain = field("universe_domain").map(str::to_string);
	let quota_project_id = json.get("quota_project_id").and_then(Value::as_str).map(str::to_string);
	match json.get("type").and_then(Value::as_str) {
		Some("authorized_user") => {
			let client_id = field("client_id")
				.ok_or_else(|| PiError::new("The incoming JSON object does not contain a client_id field"))?;
			let client_secret = field("client_secret")
				.ok_or_else(|| PiError::new("The incoming JSON object does not contain a client_secret field"))?;
			let refresh_token = field("refresh_token")
				.ok_or_else(|| PiError::new("The incoming JSON object does not contain a refresh_token field"))?;
			let mut client = AuthClient::new(
				Credential::AuthorizedUser {
					client_id: client_id.to_string(),
					client_secret: client_secret.to_string(),
				},
				scopes,
				quota_project_id,
				universe_domain,
			);
			client.credentials.get_mut().refresh_token = Some(refresh_token.to_string());
			Ok(client)
		}
		Some(kind @ ("impersonated_service_account" | "external_account" | "external_account_authorized_user")) => {
			Err(PiError::new(format!("{UNSUPPORTED_CREDENTIAL_TYPE}{kind}")))
		}
		_ => {
			let email = field("client_email")
				.ok_or_else(|| PiError::new("The incoming JSON object does not contain a client_email field"))?;
			let key = field("private_key")
				.ok_or_else(|| PiError::new("The incoming JSON object does not contain a private_key field"))?;
			Ok(AuthClient::new(
				Credential::ServiceAccount(ServiceAccount {
					email: Some(email.to_string()),
					key: key.to_string(),
					key_id: field("private_key_id").map(str::to_string),
				}),
				scopes,
				quota_project_id,
				universe_domain,
			))
		}
	}
}

struct ServiceAccount {
	email: Option<String>,
	key: String,
	key_id: Option<String>,
}

enum Credential {
	/// `JWT` from a service-account JSON key.
	ServiceAccount(ServiceAccount),
	/// The `keyFilename` fallback `new JWT({ keyFile })`: it has no scopes, so `getRequestMetadataAsync` signs a
	/// self-signed JWT with an undefined key, which `jws.sign` rejects.
	UnsignedKeyFile,
	/// `UserRefreshClient`.
	AuthorizedUser { client_id: String, client_secret: String },
	/// `Compute` (GCE metadata server).
	Compute,
}

/// `Credentials` of an `OAuth2Client`.
#[derive(Clone, Default)]
struct Tokens {
	access_token: Option<String>,
	token_type: Option<String>,
	expiry_date: Option<f64>,
	refresh_token: Option<String>,
}

struct AuthClient {
	credential: Credential,
	scopes: Vec<String>,
	quota_project_id: Option<String>,
	universe_domain: String,
	credentials: RefCell<Tokens>,
	self_signed: RefCell<Option<(f64, String)>>,
}

impl AuthClient {
	fn new(
		credential: Credential,
		scopes: Vec<String>,
		quota_project_id: Option<String>,
		universe_domain: Option<String>,
	) -> Self {
		let placeholder = match credential {
			Credential::ServiceAccount(_) | Credential::UnsignedKeyFile => Some("jwt-placeholder"),
			Credential::Compute => Some("compute-placeholder"),
			Credential::AuthorizedUser { .. } => None,
		};
		Self {
			credential,
			scopes,
			quota_project_id,
			universe_domain: universe_domain.unwrap_or_else(|| DEFAULT_UNIVERSE.into()),
			credentials: RefCell::new(Tokens {
				expiry_date: placeholder.map(|_| 1.0),
				refresh_token: placeholder.map(str::to_string),
				..Tokens::default()
			}),
			self_signed: RefCell::new(None),
		}
	}

	/// `getRequestHeaders(url)` (`JWT.getRequestMetadataAsync` or `OAuth2Client.getRequestMetadataAsync`). Inside the
	/// default universe a service account with scopes uses the OAuth token, so the URL never matters.
	async fn get_request_headers(&self, _url: &str) -> PiResult<Vec<(String, String)>> {
		if let Credential::ServiceAccount(account) = &self.credential
			&& self.universe_domain != DEFAULT_UNIVERSE
		{
			let authorization = self.self_signed_jwt(account)?;
			return Ok(self.add_shared_metadata_headers(vec![("authorization".into(), authorization)]));
		}
		let credentials = self.credentials.borrow().clone();
		if credentials.access_token.is_none() && credentials.refresh_token.is_none() {
			return Err(PiError::new(
				"No access, refresh token, API key or refresh handler callback is set.",
			));
		}
		if let Some(access_token) = &credentials.access_token
			&& !self.is_token_expiring()
		{
			let token_type = credentials.token_type.clone().unwrap_or_else(|| "Bearer".into());
			self.credentials.borrow_mut().token_type = Some(token_type.clone());
			let authorization = format!("{token_type} {access_token}");
			return Ok(self.add_shared_metadata_headers(vec![("authorization".into(), authorization)]));
		}
		let mut tokens = self
			.refresh_token_no_cache(credentials.refresh_token.as_deref())
			.await
			.map_err(|mut error| {
				if matches!(error.status, Some(403 | 404)) {
					error.message = format!("Could not refresh access token: {}", error.message);
				}
				error
			})?;
		let token_type = credentials.token_type.clone().unwrap_or_else(|| "Bearer".into());
		tokens.refresh_token = credentials.refresh_token.clone();
		let access_token = tokens.access_token.clone().unwrap_or_else(|| "undefined".into());
		*self.credentials.borrow_mut() = tokens;
		Ok(self.add_shared_metadata_headers(vec![("authorization".into(), format!("{token_type} {access_token}"))]))
	}

	/// `isTokenExpiring()`.
	fn is_token_expiring(&self) -> bool {
		self.credentials
			.borrow()
			.expiry_date
			.is_some_and(|expiry| expiry != 0.0 && expiry <= pi_host::now_ms() + DEFAULT_EAGER_REFRESH_THRESHOLD_MILLIS)
	}

	/// `addSharedMetadataHeaders(headers)`.
	fn add_shared_metadata_headers(&self, mut headers: Vec<(String, String)>) -> Vec<(String, String)> {
		if let Some(quota_project_id) = self.quota_project_id.as_ref().filter(|value| !value.is_empty()) {
			headers.push(("x-goog-user-project".into(), quota_project_id.clone()));
		}
		headers
	}

	async fn refresh_token_no_cache(&self, refresh_token: Option<&str>) -> PiResult<Tokens> {
		match &self.credential {
			Credential::ServiceAccount(account) => self.fetch_jwt_token(account).await,
			Credential::UnsignedKeyFile => Err(PiError::type_error("key must be a string, a buffer or an object")),
			Credential::AuthorizedUser {
				client_id,
				client_secret,
			} => {
				let Some(refresh_token) = refresh_token else {
					return Err(PiError::new("No refresh token is set."));
				};
				refresh_user_token(refresh_token, client_id, client_secret).await
			}
			Credential::Compute => self.fetch_compute_token().await,
		}
	}

	/// `JWT.refreshTokenNoCache` through `gtoken`: exchanges a signed assertion for an access token.
	async fn fetch_jwt_token(&self, account: &ServiceAccount) -> PiResult<Tokens> {
		let iat = (pi_host::now_ms() / 1000.0).floor();
		let mut payload = Map::new();
		if let Some(email) = &account.email {
			payload.insert("iss".into(), json!(email));
		}
		payload.insert("scope".into(), json!(self.scopes.join(" ")));
		payload.insert("aud".into(), json!(GOOGLE_TOKEN_URL));
		payload.insert("exp".into(), json!(iat + 3600.0));
		payload.insert("iat".into(), json!(iat));
		let assertion = jws_sign(&json!({ "alg": "RS256" }), &Value::Object(payload), &account.key)?;
		let body = form_urlencoded(&[("grant_type", GOOGLE_GRANT_TYPE), ("assertion", &assertion)]);
		let response = gaxios_request(GaxiosOptions {
			method: "POST",
			url: GOOGLE_TOKEN_URL.into(),
			headers: with_auth_client_headers(vec![
				("content-type", "application/x-www-form-urlencoded;charset=UTF-8".into()),
				("accept", "application/json".into()),
			]),
			body: Some(body),
			response_type: ResponseType::Json,
			retry: Some(RetryConfig {
				http_methods_to_retry: &["POST"],
				no_response_retries: 2,
			}),
			timeout_ms: 0.0,
		})
		.await
		.map_err(|error| {
			let mut pi_error = error.to_pi_error();
			if let Some(data) = error
				.data
				.as_ref()
				.filter(|data| data.get("error").is_some_and(js_truthy))
			{
				pi_error.message = format!(
					"{}: {}",
					js_string(data.get("error")),
					js_string(data.get("error_description"))
				);
			}
			pi_error
		})?;
		let data = response.data;
		let expires_in = data.get("expires_in").map_or(0.0, js_number);
		Ok(Tokens {
			access_token: data.get("access_token").and_then(Value::as_str).map(str::to_string),
			token_type: Some("Bearer".into()),
			expiry_date: Some(pi_host::now_ms() + expires_in * 1000.0),
			refresh_token: None,
		})
	}

	/// `Compute.refreshTokenNoCache`: the default service account's token from the metadata server.
	async fn fetch_compute_token(&self) -> PiResult<Tokens> {
		let mut url = format!("{}/instance/service-accounts/default/token", metadata_base_url(None));
		if !self.scopes.is_empty() {
			url.push('?');
			url.push_str(&form_urlencoded(&[("scopes", &self.scopes.join(","))]));
		}
		let data = metadata_accessor(url, 3, false).await.map_err(|error| {
			let Some(gaxios) = error.gaxios else {
				return error.error;
			};
			let mut message = format!("Could not refresh access token: {}", gaxios.message);
			match gaxios.status {
				Some(403) => {
					message = format!(
						"A Forbidden error was returned while attempting to retrieve an access token for the Compute Engine built-in service account. This may be because the Compute Engine instance does not have the correct permission scopes specified: {message}"
					);
				}
				Some(404) => {
					message = format!(
						"A Not Found error was returned while attempting to retrieve an accesstoken for the Compute Engine built-in service account. This may be because the Compute Engine instance does not have any permission scopes specified: {message}"
					);
				}
				_ => {}
			}
			GaxiosError { message, ..gaxios }.to_pi_error()
		})?;
		Ok(tokens_from_response(&data))
	}

	/// `JWTAccess.getRequestHeaders(url, undefined, scopes)`: a self-signed JWT, cached until 5 minutes before expiry.
	fn self_signed_jwt(&self, account: &ServiceAccount) -> PiResult<String> {
		let now = pi_host::now_ms();
		if let Some((expiration, authorization)) = self.self_signed.borrow().as_ref()
			&& expiration - now > DEFAULT_EAGER_REFRESH_THRESHOLD_MILLIS
		{
			return Ok(authorization.clone());
		}
		let iat = (now / 1000.0).floor();
		let exp = iat + 3600.0;
		let mut header = Map::new();
		header.insert("alg".into(), json!("RS256"));
		header.insert("typ".into(), json!("JWT"));
		if let Some(key_id) = account.key_id.as_ref().filter(|key_id| !key_id.is_empty()) {
			header.insert("kid".into(), json!(key_id));
		}
		let mut payload = Map::new();
		payload.insert("iss".into(), json!(account.email));
		payload.insert("sub".into(), json!(account.email));
		payload.insert("scope".into(), json!(self.scopes.join(" ")));
		payload.insert("exp".into(), json!(exp));
		payload.insert("iat".into(), json!(iat));
		let authorization = format!(
			"Bearer {}",
			jws_sign(&Value::Object(header), &Value::Object(payload), &account.key)?
		);
		*self.self_signed.borrow_mut() = Some((exp * 1000.0, authorization.clone()));
		Ok(authorization)
	}
}

/// `AuthClient.DEFAULT_REQUEST_INTERCEPTOR` of the client's transporter: `x-goog-api-client` and `User-Agent` when absent.
fn with_auth_client_headers(mut headers: Vec<(&'static str, String)>) -> Vec<(&'static str, String)> {
	if !headers.iter().any(|(name, _)| *name == "x-goog-api-client") {
		headers.push((
			"x-goog-api-client",
			format!("gl-rust/{}", env!("CARGO_PKG_RUST_VERSION")),
		));
	}
	headers.push(("user-agent", AUTH_LIBRARY_USER_AGENT.into()));
	headers
}

/// `UserRefreshClient.refreshTokenNoCache`: a `refresh_token` grant.
async fn refresh_user_token(refresh_token: &str, client_id: &str, client_secret: &str) -> PiResult<Tokens> {
	let body = form_urlencoded(&[
		("refresh_token", refresh_token),
		("client_id", client_id),
		("client_secret", client_secret),
		("grant_type", "refresh_token"),
	]);
	let response = gaxios_request(GaxiosOptions {
		method: "POST",
		url: GOOGLE_TOKEN_URL.into(),
		headers: with_auth_client_headers(vec![
			("content-type", "application/x-www-form-urlencoded;charset=UTF-8".into()),
			(
				"x-goog-api-client",
				format!("gl-rust/{}", env!("CARGO_PKG_RUST_VERSION")),
			),
		]),
		body: Some(body),
		response_type: ResponseType::Unknown,
		retry: Some(RetryConfig {
			http_methods_to_retry: &["GET", "PUT", "POST", "HEAD", "OPTIONS", "DELETE"],
			no_response_retries: 2,
		}),
		timeout_ms: 0.0,
	})
	.await
	.map_err(|error| {
		let mut pi_error = error.to_pi_error();
		let description = error
			.data
			.as_ref()
			.and_then(|data| data.get("error_description"))
			.and_then(Value::as_str);
		if error.message == "invalid_grant"
			&& description.is_some_and(|description| description.to_lowercase().contains("reauth"))
			&& let Some(data) = &error.data
		{
			pi_error.message = json_stringify(data);
		}
		pi_error
	})?;
	Ok(tokens_from_response(&response.data))
}

/// The token response: `expires_in` becomes `expiry_date`.
fn tokens_from_response(data: &Value) -> Tokens {
	let expires_in = data.get("expires_in").filter(|value| js_truthy(value)).map(js_number);
	Tokens {
		access_token: data.get("access_token").and_then(Value::as_str).map(str::to_string),
		token_type: data.get("token_type").and_then(Value::as_str).map(str::to_string),
		expiry_date: expires_in.map(|expires_in| pi_host::now_ms() + expires_in * 1000.0),
		refresh_token: None,
	}
}

/// `jws.sign({ header, payload, secret })` with RS256.
fn jws_sign(header: &Value, payload: &Value, key: &str) -> PiResult<String> {
	let signing_input = format!(
		"{}.{}",
		URL_SAFE_NO_PAD.encode(json_stringify(header)),
		URL_SAFE_NO_PAD.encode(json_stringify(payload))
	);
	let private_key = RsaPrivateKey::from_pkcs8_pem(key)
		.or_else(|_| RsaPrivateKey::from_pkcs1_pem(key))
		.map_err(|error| PiError::new(format!("Invalid private key: {error}")))?;
	let signature = SigningKey::<Sha256>::new(private_key).sign(signing_input.as_bytes());
	Ok(format!(
		"{signing_input}.{}",
		URL_SAFE_NO_PAD.encode(signature.to_bytes())
	))
}

/// `new URLSearchParams(pairs).toString()`.
fn form_urlencoded(pairs: &[(&str, &str)]) -> String {
	url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(pairs)
		.finish()
}

/// `value * 1000`'s numeric coercion of a JSON value.
fn js_number(value: &Value) -> f64 {
	match value {
		Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
		Value::String(text) => string_to_number(text),
		Value::Bool(value) => f64::from(u8::from(*value)),
		Value::Null => 0.0,
		Value::Array(_) | Value::Object(_) => string_to_number(&google_client::js_string(value)),
	}
}

/// A template-literal substitution of `value`.
fn js_string(value: Option<&Value>) -> String {
	value.map_or_else(|| "undefined".into(), google_client::js_string)
}

// -- gcp-metadata -------------------------------------------------------------------------------------------------

thread_local! {
	/// `gcpResidencyCache`.
	static GCP_RESIDENCY: RefCell<Option<bool>> = const { RefCell::new(None) };
	/// `cachedIsAvailableResponse`: whether the first metadata ping succeeded.
	static IS_AVAILABLE: RefCell<Option<bool>> = const { RefCell::new(None) };
}

/// `_checkIsGCE()`: `getGCPResidency() || await isAvailable()`.
async fn check_is_gce() -> PiResult<bool> {
	if gcp_residency().await {
		return Ok(true);
	}
	is_available().await
}

/// `getGCPResidency()`: serverless env vars, or a Google BIOS vendor on Linux.
async fn gcp_residency() -> bool {
	if let Some(residency) = GCP_RESIDENCY.with(|cache| *cache.borrow()) {
		return residency;
	}
	let serverless = ["CLOUD_RUN_JOB", "FUNCTION_NAME", "K_SERVICE"]
		.iter()
		.any(|name| pi_host::env(name).is_some_and(|value| !value.is_empty()));
	let compute_engine = cfg!(target_os = "linux")
		&& pi_host::file_exists("/sys/class/dmi/id/bios_date")
		&& pi_host::read_file("/sys/class/dmi/id/bios_vendor")
			.await
			.is_ok_and(|vendor| vendor.contains("Google"));
	let residency = serverless || compute_engine;
	GCP_RESIDENCY.with(|cache| *cache.borrow_mut() = Some(residency));
	residency
}

/// `detectGCPAvailableRetries()`: `Number(DETECT_GCP_RETRIES)` when set, else 0. gaxios stops a no-response retry once
/// the attempt count reaches it, so a fraction rounds up, a negative value means no retries, and NaN never stops it (the
/// 3-retry cap still applies).
fn detect_gcp_available_retries() -> u32 {
	let retries = pi_host::env("DETECT_GCP_RETRIES")
		.filter(|value| !value.is_empty())
		.map_or(0.0, |value| string_to_number(&value));
	if retries.is_nan() {
		u32::MAX
	} else if retries > 0.0 {
		retries.ceil().min(f64::from(u32::MAX)) as u32
	} else {
		0
	}
}

/// `isAvailable()`.
async fn is_available() -> PiResult<bool> {
	if let Some(value) = pi_host::env("METADATA_SERVER_DETECTION").filter(|value| !value.is_empty()) {
		let value = js_trim(&value).to_lowercase();
		match value.as_str() {
			"assume-present" => return Ok(true),
			"none" => return Ok(false),
			"bios-only" => return Ok(gcp_residency().await),
			"ping-only" => {}
			_ => {
				return Err(PiError::named(
					"RangeError",
					format!(
						"Unknown `METADATA_SERVER_DETECTION` env variable. Got `{value}`, but it should be `assume-present`, `none`, `bios-only`, `ping-only`, or unset"
					),
				));
			}
		}
	}
	if let Some(available) = IS_AVAILABLE.with(|cache| *cache.borrow()) {
		return Ok(available);
	}
	let fast_fail = !metadata_host_overridden();
	let available = metadata_accessor(
		format!("{}/instance", metadata_base_url(None)),
		detect_gcp_available_retries(),
		fast_fail,
	)
	.await
	.is_ok();
	IS_AVAILABLE.with(|cache| *cache.borrow_mut() = Some(available));
	Ok(available)
}

fn metadata_host_overridden() -> bool {
	["GCE_METADATA_IP", "GCE_METADATA_HOST"]
		.iter()
		.any(|name| pi_host::env(name).is_some_and(|value| !value.is_empty()))
}

/// `getBaseUrl(baseUrl)` of gcp-metadata.
fn metadata_base_url(base_url: Option<&str>) -> String {
	let base_url = match base_url {
		Some(base_url) => base_url.to_string(),
		None => pi_host::env("GCE_METADATA_IP")
			.filter(|value| !value.is_empty())
			.or_else(|| pi_host::env("GCE_METADATA_HOST").filter(|value| !value.is_empty()))
			.unwrap_or_else(|| METADATA_HOST_ADDRESS.into()),
	};
	let base_url = if base_url.starts_with("http://") || base_url.starts_with("https://") {
		base_url
	} else {
		format!("http://{base_url}")
	};
	url::Url::parse(&base_url)
		.and_then(|base| base.join(METADATA_BASE_PATH))
		.map(|url| url.to_string())
		.unwrap_or_else(|_| format!("{base_url}{METADATA_BASE_PATH}"))
}

struct MetadataError {
	error: PiError,
	gaxios: Option<GaxiosError>,
}

/// `metadataAccessor(type, options, noResponseRetries, fastFail)`: a metadata GET that requires `Metadata-Flavor: Google`.
async fn metadata_accessor(url: String, no_response_retries: u32, fast_fail: bool) -> Result<Value, MetadataError> {
	let timeout_ms = if gcp_residency().await { 0.0 } else { 3000.0 };
	let options = |url: String| GaxiosOptions {
		method: "GET",
		url,
		headers: vec![("metadata-flavor", "Google".into())],
		body: None,
		response_type: ResponseType::Text,
		retry: Some(RetryConfig {
			http_methods_to_retry: &["GET", "HEAD", "PUT", "OPTIONS", "DELETE"],
			no_response_retries,
		}),
		timeout_ms,
	};
	let result = if fast_fail {
		let secondary = url.replace(
			&metadata_base_url(None),
			&metadata_base_url(Some(METADATA_SECONDARY_HOST_ADDRESS)),
		);
		let primary = Box::pin(gaxios_request(options(url)));
		let secondary = Box::pin(gaxios_request(options(secondary)));
		futures::future::select_ok([primary, secondary])
			.await
			.map(|(response, _)| response)
	} else {
		gaxios_request(options(url)).await
	};
	let response = result.map_err(|gaxios| MetadataError {
		error: gaxios.to_pi_error(),
		gaxios: Some(gaxios),
	})?;
	let flavor = response.headers.get("metadata-flavor").cloned();
	if flavor.as_deref() != Some("Google") {
		let got = flavor
			.map(|flavor| format!("'{flavor}'"))
			.unwrap_or_else(|| "no header".into());
		return Err(MetadataError {
			error: PiError::named(
				"RangeError",
				format!(
					"Invalid response from metadata service: incorrect Metadata-Flavor header. Expected 'Google', got {got}"
				),
			),
			gaxios: None,
		});
	}
	match &response.data {
		Value::String(text) => Ok(serde_json::from_str::<Value>(text).unwrap_or_else(|_| response.data.clone())),
		other => Ok(other.clone()),
	}
}

// -- gaxios -------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResponseType {
	Json,
	Text,
	Unknown,
}

struct RetryConfig {
	http_methods_to_retry: &'static [&'static str],
	no_response_retries: u32,
}

struct GaxiosOptions {
	method: &'static str,
	url: String,
	headers: Vec<(&'static str, String)>,
	body: Option<String>,
	response_type: ResponseType,
	retry: Option<RetryConfig>,
	timeout_ms: f64,
}

struct GaxiosResponse {
	headers: IndexMap<String, String>,
	data: Value,
}

/// A `GaxiosError`: `status` and `data` come from the response, when there was one.
struct GaxiosError {
	message: String,
	status: Option<u16>,
	data: Option<Value>,
	aborted: bool,
}

impl GaxiosError {
	/// The thrown error as pi sees it: every `GaxiosError` has an own `status` property, so `retryGoogleRequest`
	/// treats it as a provider error even without a response.
	fn to_pi_error(&self) -> PiError {
		let error = PiError::named("GaxiosError", self.message.clone()).with_data(json!({ "headers": null }));
		match self.status {
			Some(status) => error.with_status(status),
			None => error,
		}
	}
}

/// `gaxios.request(options)` with its retry policy: 3 retries for 1xx/408/429/5xx and up to `noResponseRetries` for
/// network errors, after 100 ms, 500 ms and 1500 ms.
async fn gaxios_request(options: GaxiosOptions) -> Result<GaxiosResponse, GaxiosError> {
	let mut attempt: i32 = 0;
	loop {
		let error = match gaxios_send(&options).await {
			Ok(response) => return Ok(response),
			Err(error) => error,
		};
		if !should_retry(&options, &error, attempt) {
			return Err(error);
		}
		let delay = if attempt == 0 { 100.0 } else { 0.0 } + (2f64.powi(attempt) - 1.0) / 2.0 * 1000.0;
		pi_host::current().sleep(delay).await;
		attempt += 1;
	}
}

fn should_retry(options: &GaxiosOptions, error: &GaxiosError, attempt: i32) -> bool {
	let Some(retry) = &options.retry else {
		return false;
	};
	if error.aborted {
		return false;
	}
	if error.status.is_none() && attempt >= retry.no_response_retries as i32 {
		return false;
	}
	if !retry.http_methods_to_retry.contains(&options.method) {
		return false;
	}
	if let Some(status) = error.status
		&& !matches!(status, 100..=199 | 408 | 429 | 500..=599)
	{
		return false;
	}
	attempt < 3
}

async fn gaxios_send(options: &GaxiosOptions) -> Result<GaxiosResponse, GaxiosError> {
	// node-fetch rejects any aborted request, the timeout included, with `AbortError: The operation was aborted.`.
	let failed = |error: PiError| {
		let aborted = error.is_abort() || error.name == "TimeoutError";
		GaxiosError {
			aborted,
			message: if aborted {
				"The operation was aborted.".into()
			} else {
				error.message
			},
			status: None,
			data: None,
		}
	};
	let mut request = HttpRequest::new(options.method, options.url.clone());
	for (name, value) in &options.headers {
		request.set_header(*name, value.clone());
	}
	request.body = options.body.clone().map(Into::into);
	if options.timeout_ms > 0.0 {
		request.signal = Some(AbortSignal::timeout(options.timeout_ms));
	}
	let response = pi_host::fetch(request, None).await.map_err(failed)?;
	let status = response.status;
	let headers = response.headers.clone();
	let text = response.text().await.map_err(failed)?;
	let data = match options.response_type {
		ResponseType::Text => Value::String(text),
		ResponseType::Json | ResponseType::Unknown => match serde_json::from_str::<Value>(&text) {
			Ok(mut json) => {
				js_key_order(&mut json);
				json
			}
			Err(_) => Value::String(text),
		},
	};
	if (200..300).contains(&status) {
		return Ok(GaxiosResponse { headers, data });
	}
	Err(GaxiosError {
		message: api_error_message(&data, status),
		status: Some(status),
		data: Some(data),
		aborted: false,
	})
}

/// `GaxiosError.extractAPIErrorFromResponse(res, "Request failed with status code <status>").message`.
fn api_error_message(data: &Value, status: u16) -> String {
	let mut message = format!("Request failed with status code {status}");
	if let Value::String(text) = data {
		message = text.clone();
	}
	match data.get("error") {
		Some(Value::String(error)) if !error.is_empty() => error.clone(),
		Some(Value::Object(error)) => {
			if let Some(Value::Array(errors)) = error.get("errors") {
				let messages: Vec<&str> = errors
					.iter()
					.filter_map(|item| item.get("message").and_then(Value::as_str))
					.collect();
				if !messages.is_empty() {
					message = messages.join("\n");
				}
			}
			// `Object.assign({ message, ... }, res.data.error)`: the error's own `message` key wins, coerced by `new Error`.
			error.get("message").map_or(message, google_client::js_string)
		}
		_ => message,
	}
}
