//! Port of `packages/mcp/src/oauth/discovery.ts`.
//! PORT-STATUS: reviewed
//!
//! Adapted from modelcontextprotocol/typescript-sdk v1.29.0 src/client/auth.ts. Copyright (c) 2024 Anthropic,
//! PBC. Licensed under MIT; see LICENSES/. Modified to remove Zod/CORS shims and enforce authorization-server
//! issuer validation.
//!
//! `string | URL` parameters are `&str`; a string that is not a URL fails like `new URL()`, with a `TypeError`.
//! A discarded response is dropped, which cancels its body.

use pi_host::{AbortSignal, HttpRequest, HttpResponse, PiError, PiResult};
use url::Url;

use super::errors::OAuthIssuerMismatchError;
use super::types::{
	AuthorizationServerMetadata, OAuthChallenge, OAuthProtectedResourceMetadata, OAuthServerInfo,
	parse_authorization_server_metadata, parse_protected_resource_metadata,
};
use crate::auth_provider::McpFetch;
use crate::protocol::types::LATEST_PROTOCOL_VERSION;

/// `new URL(value)`.
pub(crate) fn parse_url(value: &str) -> PiResult<Url> {
	Url::parse(value).map_err(|_| PiError::type_error("Invalid URL"))
}

/// `new URL(path, base)`.
pub(crate) fn join_url(base: &Url, path: &str) -> PiResult<Url> {
	base.join(path).map_err(|_| PiError::type_error("Invalid URL"))
}

/// 4xx and 502 mean "not here", so discovery tries the next candidate URL.
fn is_discovery_miss(status: u16) -> bool {
	(400..500).contains(&status) || status == 502
}

/// Path suffix for `/.well-known/<kind><path>`; empty for the root path.
fn path_suffix(pathname: &str) -> &str {
	pathname.strip_suffix('/').unwrap_or(pathname)
}

/// `url.origin` as a base URL.
fn origin(url: &Url) -> PiResult<Url> {
	parse_url(&url.origin().ascii_serialization())
}

fn field(header: &str, name: &str) -> Option<String> {
	let pattern =
		regex::Regex::new(&format!(r#"(?i)(?:^|[,\s]){name}=(?:"([^"]*)"|([^\s,]+))"#)).expect("valid pattern");
	let captures = pattern.captures(header)?;
	// An empty value (`scope=""`) carries no information, so it counts as absent.
	[captures.get(1), captures.get(2)]
		.into_iter()
		.flatten()
		.map(|value| value.as_str())
		.find(|value| !value.is_empty())
		.map(str::to_string)
}

/// `parseWwwAuthenticate(header)`.
pub fn parse_www_authenticate(header: Option<&str>) -> OAuthChallenge {
	let Some(header) = header.filter(|header| !header.is_empty()) else {
		return OAuthChallenge::default();
	};
	let scheme = header.split_whitespace().next().unwrap_or("").to_lowercase();
	if scheme != "bearer" && scheme != "dpop" {
		return OAuthChallenge::default();
	}
	OAuthChallenge {
		resource_metadata_url: field(header, "resource_metadata").and_then(|url| Url::parse(&url).ok()),
		scope: field(header, "scope"),
		error: field(header, "error"),
		error_description: field(header, "error_description"),
	}
}

async fn fetch_metadata(
	url: &Url,
	fetch: Option<&McpFetch>,
	protocol_version: &str,
	signal: Option<&AbortSignal>,
) -> PiResult<HttpResponse> {
	pi_host::fetch(
		HttpRequest {
			method: "GET".into(),
			url: url.to_string(),
			headers: [
				("Accept".to_string(), "application/json".to_string()),
				("MCP-Protocol-Version".to_string(), protocol_version.to_string()),
			]
			.into_iter()
			.collect(),
			body: None,
			signal: signal.cloned(),
		},
		fetch,
	)
	.await
}

#[derive(Clone, Default)]
pub struct DiscoverProtectedResourceMetadataOptions {
	pub resource_metadata_url: Option<Url>,
	pub protocol_version: Option<String>,
	pub fetch: Option<McpFetch>,
	pub signal: Option<AbortSignal>,
}

/// `discoverProtectedResourceMetadata(serverUrl, options)`.
pub async fn discover_protected_resource_metadata(
	server_url: &str,
	options: DiscoverProtectedResourceMetadataOptions,
) -> PiResult<OAuthProtectedResourceMetadata> {
	let server = parse_url(server_url)?;
	let fetch = options.fetch.as_ref();
	let signal = options.signal.as_ref();
	let version = options.protocol_version.as_deref().unwrap_or(LATEST_PROTOCOL_VERSION);
	let url = match &options.resource_metadata_url {
		Some(url) => url.clone(),
		None => join_url(
			&origin(&server)?,
			&format!("/.well-known/oauth-protected-resource{}", path_suffix(server.path())),
		)?,
	};
	let mut response = fetch_metadata(&url, fetch, version, signal).await?;
	if options.resource_metadata_url.is_none() && server.path() != "/" && is_discovery_miss(response.status) {
		drop(response);
		let url = join_url(&origin(&server)?, "/.well-known/oauth-protected-resource")?;
		response = fetch_metadata(&url, fetch, version, signal).await?;
	}
	if !response.ok() {
		return Err(PiError::new(format!(
			"HTTP {} loading OAuth protected resource metadata",
			response.status
		)));
	}
	parse_protected_resource_metadata(response.json().await?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryUrlType {
	Oauth,
	Oidc,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveryUrl {
	pub url: Url,
	pub kind: DiscoveryUrlType,
}

/// `buildAuthorizationServerDiscoveryUrls(authorizationServerUrl)`.
pub fn build_authorization_server_discovery_urls(authorization_server_url: &str) -> PiResult<Vec<DiscoveryUrl>> {
	let issuer = parse_url(authorization_server_url)?;
	let origin = origin(&issuer)?;
	let path = path_suffix(issuer.path());
	let mut urls = vec![
		DiscoveryUrl {
			url: join_url(&origin, &format!("/.well-known/oauth-authorization-server{path}"))?,
			kind: DiscoveryUrlType::Oauth,
		},
		DiscoveryUrl {
			url: join_url(&origin, &format!("/.well-known/openid-configuration{path}"))?,
			kind: DiscoveryUrlType::Oidc,
		},
	];
	if !path.is_empty() {
		urls.push(DiscoveryUrl {
			url: join_url(&origin, &format!("{path}/.well-known/openid-configuration"))?,
			kind: DiscoveryUrlType::Oidc,
		});
	}
	Ok(urls)
}

#[derive(Clone, Default)]
pub struct DiscoverAuthorizationServerMetadataOptions {
	pub fetch: Option<McpFetch>,
	pub protocol_version: Option<String>,
	pub skip_issuer_validation: Option<bool>,
	pub signal: Option<AbortSignal>,
}

/// `discoverAuthorizationServerMetadata(authorizationServerUrl, options)`.
pub async fn discover_authorization_server_metadata(
	authorization_server_url: &str,
	options: DiscoverAuthorizationServerMetadataOptions,
) -> PiResult<Option<AuthorizationServerMetadata>> {
	let version = options.protocol_version.as_deref().unwrap_or(LATEST_PROTOCOL_VERSION);
	for DiscoveryUrl { url, .. } in build_authorization_server_discovery_urls(authorization_server_url)? {
		let response = fetch_metadata(&url, options.fetch.as_ref(), version, options.signal.as_ref()).await?;
		if !response.ok() {
			let status = response.status;
			drop(response);
			if is_discovery_miss(status) {
				continue;
			}
			return Err(PiError::new(format!(
				"HTTP {status} loading authorization server metadata from {url}"
			)));
		}
		let metadata = parse_authorization_server_metadata(response.json().await?)?;
		if options.skip_issuer_validation != Some(true) {
			// URL parsing adds a trailing slash to bare origins, so compare without one on either side.
			let trim = |value: &str| value.strip_suffix('/').unwrap_or(value).to_string();
			if trim(&metadata.issuer) != trim(authorization_server_url) {
				return Err(OAuthIssuerMismatchError::new(
					Some(authorization_server_url),
					Some(&metadata.issuer),
				));
			}
		}
		return Ok(Some(metadata));
	}
	Ok(None)
}

#[derive(Clone, Default)]
pub struct DiscoverOAuthServerInfoOptions {
	pub resource_metadata_url: Option<Url>,
	/// Metadata document to use instead of discovery. It is trusted as configured, so its issuer is not checked.
	pub authorization_server_metadata_url: Option<Url>,
	pub fetch: Option<McpFetch>,
	pub skip_issuer_validation: Option<bool>,
	pub signal: Option<AbortSignal>,
}

/// `discoverOAuthServerInfo(serverUrl, options)`.
pub async fn discover_oauth_server_info(
	server_url: &str,
	options: DiscoverOAuthServerInfoOptions,
) -> PiResult<OAuthServerInfo> {
	let resource_metadata = match discover_protected_resource_metadata(
		server_url,
		DiscoverProtectedResourceMetadataOptions {
			resource_metadata_url: options.resource_metadata_url.clone(),
			protocol_version: None,
			fetch: options.fetch.clone(),
			signal: options.signal.clone(),
		},
	)
	.await
	{
		Ok(metadata) => Some(metadata),
		Err(error) if error.name == "TypeError" => return Err(error),
		Err(_) => None,
	};
	if let Some(url) = &options.authorization_server_metadata_url {
		let response = fetch_metadata(
			url,
			options.fetch.as_ref(),
			LATEST_PROTOCOL_VERSION,
			options.signal.as_ref(),
		)
		.await?;
		if !response.ok() {
			return Err(PiError::new(format!(
				"HTTP {} loading authorization server metadata from {url}",
				response.status
			)));
		}
		let metadata = parse_authorization_server_metadata(response.json().await?)?;
		return Ok(OAuthServerInfo {
			authorization_server_url: metadata.issuer.clone(),
			authorization_server_metadata: Some(metadata),
			resource_metadata,
		});
	}
	let authorization_server_url = match resource_metadata
		.as_ref()
		.and_then(|metadata| metadata.authorization_servers.as_ref()?.first())
	{
		Some(url) => url.clone(),
		None => join_url(&parse_url(server_url)?, "/")?.to_string(),
	};
	let authorization_server_metadata = discover_authorization_server_metadata(
		&authorization_server_url,
		DiscoverAuthorizationServerMetadataOptions {
			fetch: options.fetch,
			protocol_version: None,
			skip_issuer_validation: options.skip_issuer_validation,
			signal: options.signal,
		},
	)
	.await?;
	Ok(OAuthServerInfo {
		authorization_server_url,
		authorization_server_metadata,
		resource_metadata,
	})
}

/// `resourceUrlFromServerUrl(value)`.
pub fn resource_url_from_server_url(value: &str) -> PiResult<Url> {
	let mut url = parse_url(value)?;
	url.set_fragment(None);
	Ok(url)
}

/// `selectResource(serverUrl, metadata)`.
pub fn select_resource(
	server_url: &str,
	metadata: Option<&OAuthProtectedResourceMetadata>,
) -> PiResult<Option<String>> {
	let Some(metadata) = metadata else {
		return Ok(None);
	};
	let requested = resource_url_from_server_url(server_url)?;
	let configured = parse_url(&metadata.resource)?;
	let mismatch = || {
		PiError::new(format!(
			"Protected resource {} does not match MCP server {requested}",
			metadata.resource
		))
	};
	if requested.origin() != configured.origin() {
		return Err(mismatch());
	}
	let with_slash = |path: &str| {
		if path.ends_with('/') {
			path.to_string()
		} else {
			format!("{path}/")
		}
	};
	if !with_slash(requested.path()).starts_with(&with_slash(configured.path())) {
		return Err(mismatch());
	}
	Ok(Some(metadata.resource.clone()))
}
