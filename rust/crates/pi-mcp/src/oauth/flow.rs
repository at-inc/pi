//! Port of `packages/mcp/src/oauth/flow.ts`.
//! PORT-STATUS: ported
//!
//! Adapted from modelcontextprotocol/typescript-sdk v1.29.0 src/client/auth.ts. Copyright (c) 2024 Anthropic,
//! PBC. Licensed under MIT; see LICENSES/. Modified to remove SDK/Zod dependencies and use WebCrypto for PKCE.
//!
//! `OAuthClientProvider` is a trait. Its optional TS methods have default bodies that behave like the absent
//! method (`state` is `None`, `invalidateCredentials` does nothing, ...); `saveClientInformation` is optional in
//! a way the flow observes, so `can_save_client_information` reports whether the provider implements it.
//! `AddClientAuthentication` receives the token request's headers and form parameters by value and returns
//! them, instead of mutating a `Headers` and a `URLSearchParams`. Both are `IndexMap`s whose insertion keeps
//! the position of an existing key, like `set`; header names match case-insensitively but keep their casing.
//! Methods that take `string | URL` take `&str`. Random bytes come from `rand`.

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use futures::FutureExt;
use futures::future::Shared;
use indexmap::{IndexMap, IndexSet};
use pi_host::{AbortSignal, HttpRequest, LocalBoxFuture, PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

use super::discovery::{
	DiscoverAuthorizationServerMetadataOptions, DiscoverOAuthServerInfoOptions, discover_authorization_server_metadata,
	discover_oauth_server_info, join_url, parse_url, parse_www_authenticate, select_resource,
};
use super::errors::{
	McpOAuthAuthorizationRequiredError, OAuthError, OAuthInsecureEndpointError, OAuthIssuerMismatchError,
	OAuthRegistrationError,
};
use super::types::{
	AuthorizationServerMetadata, OAuthClientInformation, OAuthClientInformationFull, OAuthClientInformationMixed,
	OAuthClientMetadata, OAuthDiscoveryState, OAuthServerInfo, OAuthTokens, is_http_url, parse_client_information,
	parse_oauth_tokens,
};
use crate::auth_provider::{AuthProvider, McpFetch, UnauthorizedContext};
use crate::transports::streamable_http::set_header;

/// Headers of a token request.
pub type OAuthHeaders = IndexMap<String, String>;
/// Form parameters of a token request.
pub type OAuthParams = IndexMap<String, String>;

/// `(headers, params, url, metadata) => void | Promise<void>`, returning the updated headers and parameters.
pub type AddClientAuthentication = Rc<
	dyn Fn(
		OAuthHeaders,
		OAuthParams,
		Url,
		Option<AuthorizationServerMetadata>,
	) -> LocalBoxFuture<'static, PiResult<(OAuthHeaders, OAuthParams)>>,
>;

/// A Client ID Metadata Document: an https URL used as `client_id`, and a redirect URI it lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthClientMetadataDocument {
	pub url: String,
	pub redirect_url: String,
}

/// The `kind` of `invalidateCredentials(kind)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
	All,
	Client,
	Tokens,
	Verifier,
	Discovery,
}

pub trait OAuthClientProvider {
	fn redirect_url(&self) -> &str;

	fn client_metadata(&self) -> &OAuthClientMetadata;

	/// Client ID Metadata Document to identify as instead of registering dynamically, or `None` to register.
	/// Called when no client information is stored; the document is not stored. `metadata` is `None` when the
	/// authorization server has none; check `client_id_metadata_document_supported`.
	fn client_metadata_document(
		&self,
		_metadata: Option<&AuthorizationServerMetadata>,
	) -> Option<OAuthClientMetadataDocument> {
		None
	}

	fn state(&self) -> LocalBoxFuture<'_, PiResult<Option<String>>> {
		Box::pin(async { Ok(None) })
	}

	fn client_information(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthClientInformationMixed>>>;

	/// Whether this provider implements `saveClientInformation`; dynamic registration needs it.
	fn can_save_client_information(&self) -> bool {
		false
	}

	fn save_client_information(&self, _information: OAuthClientInformationMixed) -> LocalBoxFuture<'_, PiResult<()>> {
		Box::pin(async { Ok(()) })
	}

	fn tokens(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthTokens>>>;

	fn save_tokens(&self, tokens: OAuthTokens) -> LocalBoxFuture<'_, PiResult<()>>;

	fn redirect_to_authorization(&self, url: Url) -> LocalBoxFuture<'_, PiResult<()>>;

	fn save_code_verifier(&self, verifier: String) -> LocalBoxFuture<'_, PiResult<()>>;

	fn code_verifier(&self) -> LocalBoxFuture<'_, PiResult<String>>;

	fn add_client_authentication(&self) -> Option<AddClientAuthentication> {
		None
	}

	fn invalidate_credentials(&self, _kind: CredentialKind) -> LocalBoxFuture<'_, PiResult<()>> {
		Box::pin(async { Ok(()) })
	}

	fn save_discovery_state(&self, _state: OAuthDiscoveryState) -> LocalBoxFuture<'_, PiResult<()>> {
		Box::pin(async { Ok(()) })
	}

	fn discovery_state(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthDiscoveryState>>> {
		Box::pin(async { Ok(None) })
	}
}

#[derive(Clone, Default)]
pub struct OAuthFlowOptions {
	pub server_url: String,
	pub authorization_code: Option<String>,
	/// `iss` parameter of the authorization response that delivered `authorization_code` (RFC 9207).
	pub iss: Option<String>,
	pub scope: Option<String>,
	pub resource_metadata_url: Option<Url>,
	/// Authorization server metadata document to use instead of discovery, for servers that advertise a
	/// wrong authorization server or none. It is trusted as configured. Must use https, except on loopback.
	pub authorization_server_metadata_url: Option<Url>,
	pub fetch: Option<McpFetch>,
	/// Aborts every request of the flow. Requests have no time limit of their own; combine with a timeout as needed.
	pub signal: Option<AbortSignal>,
	pub skip_issuer_validation: Option<bool>,
	/// Go straight to the authorization redirect instead of refreshing stored tokens, for example when the
	/// server asks for scopes the current grant lacks (a refresh keeps the old scope).
	pub skip_refresh: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OAuthFlowResult {
	Authorized,
	Redirect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClientAuthMethod {
	ClientSecretBasic,
	ClientSecretPost,
	None,
}

#[derive(Clone)]
pub struct TokenRequestOptions {
	pub metadata: Option<AuthorizationServerMetadata>,
	pub client_information: OAuthClientInformationMixed,
	pub resource: Option<String>,
	pub add_client_authentication: Option<AddClientAuthentication>,
	pub fetch: Option<McpFetch>,
	pub signal: Option<AbortSignal>,
}

fn loopback(hostname: &str) -> bool {
	matches!(hostname, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// The OpenID Connect `application_type` for `redirect_uris` (MCP SEP-837). Without one, OpenID Connect servers
/// assume `web`, which rejects http loopback redirect URIs. Loopback hosts and custom schemes are native (RFC 8252).
fn application_type(redirect_uris: &[String]) -> &'static str {
	let native = redirect_uris
		.iter()
		.any(|uri| Url::parse(uri).is_ok_and(|url| !is_http_url(&url) || url.host_str().is_some_and(loopback)));
	if native { "native" } else { "web" }
}

fn secure_endpoint(url: &Url) -> PiResult<Url> {
	if url.scheme() != "https" && !url.host_str().is_some_and(loopback) {
		return Err(OAuthInsecureEndpointError::new(url.as_str()));
	}
	Ok(url.clone())
}

fn has_secret(information: &OAuthClientInformation) -> bool {
	information
		.client_secret
		.as_deref()
		.is_some_and(|secret| !secret.is_empty())
}

fn select_client_auth_method(information: &OAuthClientInformationMixed, supported: &[String]) -> ClientAuthMethod {
	let parse = |method: &str| match method {
		"client_secret_basic" => Some(ClientAuthMethod::ClientSecretBasic),
		"client_secret_post" => Some(ClientAuthMethod::ClientSecretPost),
		"none" => Some(ClientAuthMethod::None),
		_ => None,
	};
	let supports = |method: &str| supported.iter().any(|item| item == method);
	if let Some(hinted) = information
		.token_endpoint_auth_method()
		.filter(|hinted| !hinted.is_empty())
		&& let Some(method) = parse(hinted)
		&& (supported.is_empty() || supports(hinted))
	{
		return method;
	}
	let secret = has_secret(information.information());
	if supported.is_empty() {
		return if secret {
			ClientAuthMethod::ClientSecretBasic
		} else {
			ClientAuthMethod::None
		};
	}
	if secret && supports("client_secret_basic") {
		return ClientAuthMethod::ClientSecretBasic;
	}
	if secret && supports("client_secret_post") {
		return ClientAuthMethod::ClientSecretPost;
	}
	if supports("none") {
		return ClientAuthMethod::None;
	}
	if secret {
		ClientAuthMethod::ClientSecretPost
	} else {
		ClientAuthMethod::None
	}
}

fn apply_client_authentication(
	method: ClientAuthMethod,
	information: &OAuthClientInformation,
	headers: &mut OAuthHeaders,
	params: &mut OAuthParams,
) -> PiResult<()> {
	if method == ClientAuthMethod::ClientSecretBasic {
		let Some(secret) = information.client_secret.as_deref().filter(|secret| !secret.is_empty()) else {
			return Err(PiError::new("client_secret_basic requires a client secret"));
		};
		let credentials = STANDARD.encode(format!("{}:{secret}", information.client_id));
		set_header(headers, "Authorization", &format!("Basic {credentials}"));
		return Ok(());
	}
	params.insert("client_id".into(), information.client_id.clone());
	if method == ClientAuthMethod::ClientSecretPost && has_secret(information) {
		params.insert(
			"client_secret".into(),
			information.client_secret.clone().unwrap_or_default(),
		);
	}
	Ok(())
}

fn pkce() -> (String, String) {
	let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
	let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
	(verifier, challenge)
}

/// `url.searchParams.set(name, value)` for each pair, in order.
fn set_search_params(url: &mut Url, params: &[(&str, &str)]) {
	let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
	for (name, value) in params {
		match pairs.iter().position(|(key, _)| key == name) {
			Some(index) => {
				pairs[index].1 = (*value).to_string();
				let mut seen = 0;
				pairs.retain(|(key, _)| {
					if key != name {
						return true;
					}
					seen += 1;
					seen == 1
				});
			}
			None => pairs.push(((*name).to_string(), (*value).to_string())),
		}
	}
	url.query_pairs_mut().clear().extend_pairs(pairs);
}

#[derive(Clone)]
pub struct StartAuthorizationOptions {
	pub metadata: Option<AuthorizationServerMetadata>,
	pub client_information: OAuthClientInformationMixed,
	pub redirect_url: String,
	pub scope: Option<String>,
	pub state: Option<String>,
	pub resource: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AuthorizationStart {
	pub authorization_url: Url,
	pub code_verifier: String,
}

/// `startAuthorization(authorizationServerUrl, options)`.
pub async fn start_authorization(
	authorization_server_url: &str,
	options: StartAuthorizationOptions,
) -> PiResult<AuthorizationStart> {
	let metadata = options.metadata.as_ref();
	if let Some(metadata) = metadata
		&& !metadata.response_types_supported.iter().any(|kind| kind == "code")
	{
		return Err(PiError::new(
			"Authorization server does not support authorization codes",
		));
	}
	if let Some(methods) = metadata.and_then(|metadata| metadata.code_challenge_methods_supported.as_ref())
		&& !methods.iter().any(|method| method == "S256")
	{
		return Err(PiError::new("Authorization server does not support PKCE S256"));
	}
	let mut url = match metadata {
		Some(metadata) => parse_url(&metadata.authorization_endpoint)?,
		None => join_url(&parse_url(authorization_server_url)?, "/authorize")?,
	};
	// Callers open this URL with the platform URL handler, which also launches files and custom protocols.
	if !is_http_url(&url) {
		return Err(PiError::new(format!("Invalid authorization endpoint {url}")));
	}
	let (verifier, challenge) = pkce();
	let mut params = vec![
		("response_type", "code"),
		("client_id", options.client_information.information().client_id.as_str()),
		("code_challenge", challenge.as_str()),
		("code_challenge_method", "S256"),
		("redirect_uri", options.redirect_url.as_str()),
	];
	fn nonempty(value: &Option<String>) -> Option<&str> {
		value.as_deref().filter(|value| !value.is_empty())
	}
	if let Some(state) = nonempty(&options.state) {
		params.push(("state", state));
	}
	if let Some(scope) = nonempty(&options.scope) {
		params.push(("scope", scope));
	}
	if options
		.scope
		.as_deref()
		.is_some_and(|scope| scope.split_whitespace().any(|item| item == "offline_access"))
	{
		params.push(("prompt", "consent"));
	}
	if let Some(resource) = nonempty(&options.resource) {
		params.push(("resource", resource));
	}
	set_search_params(&mut url, &params);
	Ok(AuthorizationStart {
		authorization_url: url,
		code_verifier: verifier,
	})
}

async fn token_request(
	authorization_server_url: &str,
	options: &TokenRequestOptions,
	mut params: OAuthParams,
) -> PiResult<OAuthTokens> {
	let endpoint = match &options.metadata {
		Some(metadata) => parse_url(&metadata.token_endpoint)?,
		None => join_url(&parse_url(authorization_server_url)?, "/token")?,
	};
	let url = secure_endpoint(&endpoint)?;
	let mut headers: OAuthHeaders = [
		("Accept".to_string(), "application/json".to_string()),
		(
			"content-type".to_string(),
			"application/x-www-form-urlencoded".to_string(),
		),
	]
	.into_iter()
	.collect();
	if let Some(resource) = options.resource.as_deref().filter(|resource| !resource.is_empty()) {
		params.insert("resource".into(), resource.to_string());
	}
	match &options.add_client_authentication {
		Some(add_client_authentication) => {
			(headers, params) =
				add_client_authentication(headers, params, url.clone(), options.metadata.clone()).await?;
		}
		None => apply_client_authentication(
			select_client_auth_method(
				&options.client_information,
				options
					.metadata
					.as_ref()
					.and_then(|metadata| metadata.token_endpoint_auth_methods_supported.as_deref())
					.unwrap_or_default(),
			),
			options.client_information.information(),
			&mut headers,
			&mut params,
		)?,
	}
	let body = url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(params.iter())
		.finish();
	let response = pi_host::fetch(
		HttpRequest {
			method: "POST".into(),
			url: url.to_string(),
			headers,
			body: Some(body.into()),
			signal: options.signal.clone(),
		},
		options.fetch.as_ref(),
	)
	.await?;
	let status = response.status;
	let ok = response.ok();
	let text = response.text().await?;
	let value: Option<Value> = serde_json::from_str(&text).ok();
	// Servers may report OAuth errors with any status, so check the body before the status.
	if let Some(Value::Object(object)) = &value
		&& let Some(Value::String(error)) = object.get("error")
	{
		let description = object.get("error_description").and_then(Value::as_str).unwrap_or(error);
		let uri = object.get("error_uri").and_then(Value::as_str);
		return Err(OAuthError::new(error, description, uri));
	}
	if !ok {
		return Err(OAuthError::new("server_error", &format!("HTTP {status}: {text}"), None));
	}
	parse_oauth_tokens(value.unwrap_or(Value::Null))
}

#[derive(Clone, Default)]
pub struct RegisterClientOptions {
	pub metadata: Option<AuthorizationServerMetadata>,
	pub client_metadata: OAuthClientMetadata,
	pub scope: Option<String>,
	pub fetch: Option<McpFetch>,
	pub signal: Option<AbortSignal>,
}

/// `registerClient(authorizationServerUrl, options)`.
pub async fn register_client(
	authorization_server_url: &str,
	options: RegisterClientOptions,
) -> PiResult<OAuthClientInformationFull> {
	let endpoint = options
		.metadata
		.as_ref()
		.and_then(|metadata| metadata.registration_endpoint.clone());
	if options.metadata.is_some() && endpoint.is_none() {
		return Err(PiError::new(
			"Authorization server does not support dynamic client registration",
		));
	}
	let url = match endpoint {
		Some(endpoint) => parse_url(&endpoint)?,
		None => join_url(&parse_url(authorization_server_url)?, "/register")?,
	};
	let mut body: Map<String, Value> = serde_json::from_value(serde_json::to_value(&options.client_metadata)?)?;
	let application_type = match &options.client_metadata.application_type {
		Some(application_type) => application_type.clone(),
		None => application_type(&options.client_metadata.redirect_uris).to_string(),
	};
	body.insert("application_type".into(), Value::String(application_type));
	if let Some(scope) = options.scope.filter(|scope| !scope.is_empty()) {
		body.insert("scope".into(), Value::String(scope));
	}
	let response = pi_host::fetch(
		HttpRequest {
			method: "POST".into(),
			url: url.to_string(),
			headers: [
				("Accept".to_string(), "application/json".to_string()),
				("content-type".to_string(), "application/json".to_string()),
			]
			.into_iter()
			.collect(),
			body: Some(pi_host::js::json_stringify(&body).into()),
			signal: options.signal,
		},
		options.fetch.as_ref(),
	)
	.await?;
	if !response.ok() {
		let status = response.status;
		return Err(OAuthRegistrationError::new(status, &response.text().await?));
	}
	parse_client_information(response.json().await?)
}

/// `exchangeAuthorizationCode(authorizationServerUrl, { ...options, code, codeVerifier, redirectUrl })`.
pub async fn exchange_authorization_code(
	authorization_server_url: &str,
	options: &TokenRequestOptions,
	code: &str,
	code_verifier: &str,
	redirect_url: &str,
) -> PiResult<OAuthTokens> {
	let params = [
		("grant_type", "authorization_code"),
		("code", code),
		("code_verifier", code_verifier),
		("redirect_uri", redirect_url),
	];
	token_request(
		authorization_server_url,
		options,
		params
			.iter()
			.map(|(key, value)| (key.to_string(), value.to_string()))
			.collect(),
	)
	.await
}

/// `refreshAuthorization(authorizationServerUrl, { ...options, refreshToken })`. A response without a new
/// refresh token keeps the old one.
pub async fn refresh_authorization(
	authorization_server_url: &str,
	options: &TokenRequestOptions,
	refresh_token: &str,
) -> PiResult<OAuthTokens> {
	let params = [("grant_type", "refresh_token"), ("refresh_token", refresh_token)];
	let mut tokens = token_request(
		authorization_server_url,
		options,
		params
			.iter()
			.map(|(key, value)| (key.to_string(), value.to_string()))
			.collect(),
	)
	.await?;
	tokens.refresh_token.get_or_insert_with(|| refresh_token.to_string());
	Ok(tokens)
}

fn with_scope(mut tokens: OAuthTokens, scope: Option<&str>) -> OAuthTokens {
	if tokens.scope.is_none()
		&& let Some(scope) = scope.filter(|scope| !scope.is_empty())
	{
		tokens.scope = Some(scope.to_string());
	}
	tokens
}

/// Scopes for a step-up authorization: the challenged scopes plus the ones granted so far, since a
/// challenge may list only the missing scopes and a token with just those would lose access the old
/// one had (SEP-2350). Without challenged scopes, `None` lets the flow pick its default.
pub fn step_up_scope(granted: Option<&str>, challenged: Option<&str>) -> Option<String> {
	let challenged = challenged.filter(|scope| !scope.is_empty())?;
	let scopes: IndexSet<&str> = [granted, Some(challenged)]
		.into_iter()
		.flatten()
		.flat_map(str::split_whitespace)
		.collect();
	Some(scopes.into_iter().collect::<Vec<_>>().join(" "))
}

async fn discover<P: OAuthClientProvider + ?Sized>(
	provider: &P,
	options: &OAuthFlowOptions,
	metadata_url: Option<&Url>,
) -> PiResult<OAuthServerInfo> {
	// With a configured metadata URL, discovery is not cached, so changing the URL applies at once.
	let cached = match metadata_url {
		Some(_) => None,
		None => provider.discovery_state().await?,
	};
	let Some(cached) = cached.filter(|cached| !cached.authorization_server_url.is_empty()) else {
		return discover_oauth_server_info(
			&options.server_url,
			DiscoverOAuthServerInfoOptions {
				resource_metadata_url: options.resource_metadata_url.clone(),
				authorization_server_metadata_url: metadata_url.cloned(),
				fetch: options.fetch.clone(),
				skip_issuer_validation: options.skip_issuer_validation,
				signal: options.signal.clone(),
			},
		)
		.await;
	};
	let authorization_server_metadata = match cached.authorization_server_metadata {
		Some(metadata) => Some(metadata),
		None => {
			discover_authorization_server_metadata(
				&cached.authorization_server_url,
				DiscoverAuthorizationServerMetadataOptions {
					fetch: options.fetch.clone(),
					protocol_version: None,
					skip_issuer_validation: options.skip_issuer_validation,
					signal: options.signal.clone(),
				},
			)
			.await?
		}
	};
	Ok(OAuthServerInfo {
		authorization_server_url: cached.authorization_server_url,
		authorization_server_metadata,
		resource_metadata: cached.resource_metadata,
	})
}

async fn run_flow<P: OAuthClientProvider + ?Sized>(
	provider: &P,
	options: &OAuthFlowOptions,
) -> PiResult<OAuthFlowResult> {
	let metadata_url = options
		.authorization_server_metadata_url
		.as_ref()
		.map(secure_endpoint)
		.transpose()?;
	let discovered = discover(provider, options, metadata_url.as_ref()).await?;
	if metadata_url.is_none() {
		provider
			.save_discovery_state(OAuthDiscoveryState {
				authorization_server_url: discovered.authorization_server_url.clone(),
				authorization_server_metadata: discovered.authorization_server_metadata.clone(),
				resource_metadata: discovered.resource_metadata.clone(),
				resource_metadata_url: options.resource_metadata_url.as_ref().map(Url::to_string),
			})
			.await?;
	}
	let authorization_server_url = discovered.authorization_server_url.as_str();
	let metadata = discovered.authorization_server_metadata.clone();
	let resource = select_resource(&options.server_url, discovered.resource_metadata.as_ref())?;
	// An empty scope (for example from `scopes_supported: []`) falls through to the next source, like `||`.
	let scope = options
		.scope
		.clone()
		.filter(|scope| !scope.is_empty())
		.or_else(|| {
			let supported = discovered.resource_metadata.as_ref()?.scopes_supported.as_ref()?;
			Some(supported.join(" ")).filter(|scope| !scope.is_empty())
		})
		.or_else(|| provider.client_metadata().scope.clone());
	let stored = provider.client_information().await?;
	let client_document = match &stored {
		Some(_) => None,
		None => provider.client_metadata_document(metadata.as_ref()),
	};
	if let Some(document) = &client_document {
		let url = parse_url(&document.url)?;
		if url.scheme() != "https" || url.path() == "/" {
			return Err(PiError::new("Invalid OAuth client metadata URL"));
		}
	}
	let document_client = client_document.as_ref().map(|document| {
		OAuthClientInformationMixed::from(OAuthClientInformation {
			client_id: document.url.clone(),
			..Default::default()
		})
	});
	let client = match stored.or(document_client) {
		Some(client) => client,
		None => {
			if options
				.authorization_code
				.as_deref()
				.is_some_and(|code| !code.is_empty())
			{
				return Err(PiError::new("OAuth client information is missing during code exchange"));
			}
			if !provider.can_save_client_information() {
				return Err(PiError::new("OAuth client information cannot be persisted"));
			}
			let client = OAuthClientInformationMixed::from(
				register_client(
					authorization_server_url,
					RegisterClientOptions {
						metadata: metadata.clone(),
						client_metadata: provider.client_metadata().clone(),
						scope: scope.clone(),
						fetch: options.fetch.clone(),
						signal: options.signal.clone(),
					},
				)
				.await?,
			);
			provider.save_client_information(client.clone()).await?;
			client
		}
	};
	// The document's redirect URI may differ from the provider's, for example by a server-specific path.
	let redirect_url = match &client_document {
		Some(document) => document.redirect_url.clone(),
		None => provider.redirect_url().to_string(),
	};
	let token_options = TokenRequestOptions {
		metadata: metadata.clone(),
		client_information: client.clone(),
		resource: resource.clone(),
		add_client_authentication: provider.add_client_authentication(),
		fetch: options.fetch.clone(),
		signal: options.signal.clone(),
	};
	if let Some(code) = options.authorization_code.as_deref().filter(|code| !code.is_empty()) {
		// RFC 9207: never send a code from another authorization server to this one. Without metadata there
		// is no issuer to compare with, so any `iss` is rejected.
		let iss = options.iss.as_deref();
		let issuer = metadata.as_ref().map(|metadata| metadata.issuer.as_str());
		let promised = metadata
			.as_ref()
			.and_then(|metadata| metadata.authorization_response_iss_parameter_supported)
			== Some(true);
		if (iss.is_some() || promised) && iss != issuer {
			return Err(OAuthIssuerMismatchError::new(issuer, iss));
		}
		let code_verifier = provider.code_verifier().await?;
		let tokens = exchange_authorization_code(
			authorization_server_url,
			&token_options,
			code,
			&code_verifier,
			&redirect_url,
		)
		.await?;
		// A response without `scope` grants the requested scope (RFC 6749 `5.1). Recorded so a step-up can
		// keep it. Callers pass the options of the authorization request, so `scope` is what was requested.
		provider.save_tokens(with_scope(tokens, scope.as_deref())).await?;
		return Ok(OAuthFlowResult::Authorized);
	}
	let existing = match options.skip_refresh {
		Some(true) => None,
		_ => provider.tokens().await?,
	};
	if let Some(existing) = existing
		&& let Some(refresh_token) = existing.refresh_token.as_deref().filter(|token| !token.is_empty())
	{
		match refresh_authorization(authorization_server_url, &token_options, refresh_token).await {
			Ok(tokens) => {
				// A refresh without `scope` keeps the scope of the grant (RFC 6749 `6).
				provider
					.save_tokens(with_scope(tokens, existing.scope.as_deref()))
					.await?;
				return Ok(OAuthFlowResult::Authorized);
			}
			Err(error) => {
				if options.signal.as_ref().is_some_and(AbortSignal::aborted) || OAuthInsecureEndpointError::is(&error) {
					return Err(error);
				}
				if OAuthError::is(&error) && OAuthError::code(&error).as_deref() != Some("server_error") {
					return Err(error);
				}
			}
		}
	}
	let state = provider.state().await?;
	let authorization = start_authorization(
		authorization_server_url,
		StartAuthorizationOptions {
			metadata,
			client_information: client,
			redirect_url,
			scope,
			state,
			resource,
		},
	)
	.await?;
	provider.save_code_verifier(authorization.code_verifier).await?;
	provider
		.redirect_to_authorization(authorization.authorization_url)
		.await?;
	Ok(OAuthFlowResult::Redirect)
}

/// `authorizeMcp(provider, options)`: runs the flow, and runs it again once after invalidating the
/// credentials an `invalid_client`, `unauthorized_client`, or `invalid_grant` error rejected.
pub async fn authorize_mcp<P: OAuthClientProvider + ?Sized>(
	provider: &P,
	options: OAuthFlowOptions,
) -> PiResult<OAuthFlowResult> {
	let error = match run_flow(provider, &options).await {
		Ok(result) => return Ok(result),
		Err(error) => error,
	};
	let code = if OAuthError::is(&error) {
		OAuthError::code(&error)
	} else {
		None
	};
	let kind = match code.as_deref() {
		Some("invalid_client" | "unauthorized_client") => CredentialKind::All,
		Some("invalid_grant") => CredentialKind::Tokens,
		_ => return Err(error),
	};
	provider.invalidate_credentials(kind).await?;
	run_flow(provider, &options).await
}

type SharedFlow = Shared<LocalBoxFuture<'static, PiResult<()>>>;

/// Auth provider for `StreamableHttpTransport`. After a 401 it refreshes the tokens, or fails with
/// `McpOAuthAuthorizationRequiredError` when the user has to authorize (again). Concurrent 401s share
/// one refresh, and a request whose token was already replaced is just retried: with rotating refresh
/// tokens, a second refresh with the old refresh token would fail and discard the new grant.
pub fn adapt_oauth_provider(provider: Rc<dyn OAuthClientProvider>) -> AuthProvider {
	let in_flight: Rc<RefCell<Option<SharedFlow>>> = Rc::default();
	let token_provider = provider.clone();
	AuthProvider {
		token: Rc::new(move || {
			let provider = token_provider.clone();
			Box::pin(async move { Ok(provider.tokens().await?.map(|tokens| tokens.access_token)) })
		}),
		on_unauthorized: Some(Rc::new(move |context: UnauthorizedContext| {
			let provider = provider.clone();
			let in_flight = in_flight.clone();
			Box::pin(async move {
				let challenge = parse_www_authenticate(context.response.header("www-authenticate"));
				let insufficient_scope = challenge.error.as_deref() == Some("insufficient_scope");
				if !insufficient_scope && in_flight.borrow().is_none() && context.token.is_some() {
					let current = provider.tokens().await?.map(|tokens| tokens.access_token);
					if current.is_some() && current != context.token {
						return Ok(());
					}
				}
				let existing = in_flight.borrow().clone();
				let flow = match existing {
					Some(flow) => flow,
					None => {
						let flow = recover(provider, in_flight.clone(), context, insufficient_scope).shared();
						*in_flight.borrow_mut() = Some(flow.clone());
						flow
					}
				};
				flow.await
			})
		})),
	}
}

/// The shared recovery of `adaptOAuthProvider`; clears `in_flight` when it settles.
fn recover(
	provider: Rc<dyn OAuthClientProvider>,
	in_flight: Rc<RefCell<Option<SharedFlow>>>,
	context: UnauthorizedContext,
	insufficient_scope: bool,
) -> LocalBoxFuture<'static, PiResult<()>> {
	let challenge = parse_www_authenticate(context.response.header("www-authenticate"));
	let server_url = context.server_url.to_string();
	let fetch = context.fetch;
	Box::pin(async move {
		let result = async {
			let granted = if insufficient_scope {
				provider.tokens().await?
			} else {
				None
			};
			let scope = if insufficient_scope {
				step_up_scope(
					granted.as_ref().and_then(|tokens| tokens.scope.as_deref()),
					challenge.scope.as_deref(),
				)
			} else {
				challenge.scope.clone()
			};
			let result = authorize_mcp(
				&*provider,
				OAuthFlowOptions {
					server_url,
					resource_metadata_url: challenge.resource_metadata_url.clone(),
					scope,
					fetch: Some(fetch),
					skip_refresh: Some(insufficient_scope),
					..Default::default()
				},
			)
			.await?;
			if result == OAuthFlowResult::Redirect {
				return Err(McpOAuthAuthorizationRequiredError::new());
			}
			Ok(())
		}
		.await;
		*in_flight.borrow_mut() = None;
		result
	})
}
