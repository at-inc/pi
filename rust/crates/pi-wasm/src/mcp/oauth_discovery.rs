//! Exports of `oauth/discovery`. URLs in results are strings; the facade turns the ones the TS types as `URL` into
//! `URL` objects.

use js_sys::Promise;
use pi_mcp::oauth::OAuthProtectedResourceMetadata;
use pi_mcp::oauth::discovery::{
	DiscoverAuthorizationServerMetadataOptions, DiscoverOAuthServerInfoOptions,
	DiscoverProtectedResourceMetadataOptions, DiscoveryUrlType,
};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

use super::optional;
use crate::options::throw;

/// `parseWwwAuthenticate(header)`, with `resourceMetadataUrl` as a string.
#[wasm_bindgen(js_name = mcpParseWwwAuthenticate)]
pub fn parse_www_authenticate(header: Option<String>) -> Result<JsValue, JsValue> {
	let challenge = pi_mcp::oauth::parse_www_authenticate(header.as_deref());
	let mut object = serde_json::Map::new();
	let fields = [
		("resourceMetadataUrl", challenge.resource_metadata_url.map(String::from)),
		("scope", challenge.scope),
		("error", challenge.error),
		("errorDescription", challenge.error_description),
	];
	for (key, value) in fields {
		if let Some(value) = value {
			object.insert(key.into(), Value::String(value));
		}
	}
	throw(crate::json::to_js(&object))
}

/// `discoverProtectedResourceMetadata(serverUrl, options)`.
#[wasm_bindgen(js_name = mcpDiscoverProtectedResourceMetadata)]
pub fn discover_protected_resource_metadata(server_url: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let server_url = super::url_string(&server_url).unwrap_or_default();
		let options = DiscoverProtectedResourceMetadataOptions {
			resource_metadata_url: super::url_field(&options, "resourceMetadataUrl")?,
			protocol_version: optional(&options, "protocolVersion")?,
			fetch: super::fetch(&options),
			signal: super::signal(&options),
		};
		Ok(super::promise(async move {
			pi_mcp::oauth::discover_protected_resource_metadata(&server_url, options).await
		}))
	})
}

/// `buildAuthorizationServerDiscoveryUrls(authorizationServerUrl)`, with each `url` as a string.
#[wasm_bindgen(js_name = mcpBuildAuthorizationServerDiscoveryUrls)]
pub fn build_authorization_server_discovery_urls(authorization_server_url: JsValue) -> Result<JsValue, JsValue> {
	let url = super::url_string(&authorization_server_url).unwrap_or_default();
	let urls = throw(pi_mcp::oauth::build_authorization_server_discovery_urls(&url))?;
	let urls: Vec<Value> = urls
		.into_iter()
		.map(|entry| {
			let kind = match entry.kind {
				DiscoveryUrlType::Oauth => "oauth",
				DiscoveryUrlType::Oidc => "oidc",
			};
			json!({ "url": entry.url.as_str(), "type": kind })
		})
		.collect();
	throw(crate::json::to_js(&urls))
}

/// `discoverAuthorizationServerMetadata(authorizationServerUrl, options)`.
#[wasm_bindgen(js_name = mcpDiscoverAuthorizationServerMetadata)]
pub fn discover_authorization_server_metadata(authorization_server_url: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let url = super::url_string(&authorization_server_url).unwrap_or_default();
		let options = DiscoverAuthorizationServerMetadataOptions {
			fetch: super::fetch(&options),
			protocol_version: optional(&options, "protocolVersion")?,
			skip_issuer_validation: optional(&options, "skipIssuerValidation")?,
			signal: super::signal(&options),
		};
		Ok(super::optional_promise(async move {
			pi_mcp::oauth::discover_authorization_server_metadata(&url, options).await
		}))
	})
}

/// `discoverOAuthServerInfo(serverUrl, options)`.
#[wasm_bindgen(js_name = mcpDiscoverOAuthServerInfo)]
pub fn discover_oauth_server_info(server_url: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let server_url = super::url_string(&server_url).unwrap_or_default();
		let options = DiscoverOAuthServerInfoOptions {
			resource_metadata_url: super::url_field(&options, "resourceMetadataUrl")?,
			authorization_server_metadata_url: super::url_field(&options, "authorizationServerMetadataUrl")?,
			fetch: super::fetch(&options),
			skip_issuer_validation: optional(&options, "skipIssuerValidation")?,
			signal: super::signal(&options),
		};
		Ok(super::promise(async move {
			pi_mcp::oauth::discover_oauth_server_info(&server_url, options).await
		}))
	})
}

/// `resourceUrlFromServerUrl(value)`, as a string.
#[wasm_bindgen(js_name = mcpResourceUrlFromServerUrl)]
pub fn resource_url_from_server_url(value: JsValue) -> Result<String, JsValue> {
	let value = super::url_string(&value).unwrap_or_default();
	throw(pi_mcp::oauth::resource_url_from_server_url(&value)).map(String::from)
}

/// `selectResource(serverUrl, metadata)`.
#[wasm_bindgen(js_name = mcpSelectResource)]
pub fn select_resource(server_url: JsValue, metadata: JsValue) -> Result<Option<String>, JsValue> {
	let server_url = super::url_string(&server_url).unwrap_or_default();
	let metadata: Option<OAuthProtectedResourceMetadata> =
		super::optional_result(metadata).map_err(|error| crate::error::to_js(&error))?;
	throw(pi_mcp::oauth::select_resource(&server_url, metadata.as_ref()))
}
