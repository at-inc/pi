//! Exports of `oauth/flow`: `authorizeMcp`, `adaptOAuthProvider` and the flow steps, over providers that are
//! either the Rust `McpOAuthProvider` of the facade's class or an `OAuthClientProvider` written in JS.

use std::rc::Rc;

use js_sys::{Array, Function, Promise};
use pi_host::{LocalBoxFuture, PiError, PiResult};
use pi_mcp::oauth::flow::{
	AddClientAuthentication, CredentialKind, OAuthHeaders, OAuthParams, RegisterClientOptions,
	StartAuthorizationOptions,
};
use pi_mcp::oauth::{
	AuthorizationServerMetadata, OAuthClientInformationMixed, OAuthClientMetadata, OAuthClientMetadataDocument,
	OAuthClientProvider, OAuthDiscoveryState, OAuthFlowOptions, OAuthTokens, TokenRequestOptions,
};
use url::Url;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::auth_provider::AuthProviderHandle;
use super::oauth_provider::McpOAuthProviderHandle;
use super::{field, function, optional};

#[wasm_bindgen]
extern "C" {
	#[wasm_bindgen(js_name = Headers)]
	type JsHeaders;
	#[wasm_bindgen(constructor, catch, js_class = "Headers")]
	fn new(init: &JsValue) -> Result<JsHeaders, JsValue>;

	#[wasm_bindgen(js_name = URLSearchParams)]
	type JsSearchParams;
	#[wasm_bindgen(constructor, catch, js_class = "URLSearchParams")]
	fn new(init: &JsValue) -> Result<JsSearchParams, JsValue>;
}

/// `authorizeMcp(provider, options)`.
#[wasm_bindgen(js_name = mcpAuthorizeMcp)]
pub fn authorize_mcp(native: Option<McpOAuthProviderHandle>, js: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let provider = provider(native, js)?;
		let options = OAuthFlowOptions {
			server_url: super::url_string(&field(&options, "serverUrl")).unwrap_or_default(),
			authorization_code: optional(&options, "authorizationCode")?,
			iss: optional(&options, "iss")?,
			scope: optional(&options, "scope")?,
			resource_metadata_url: super::url_field(&options, "resourceMetadataUrl")?,
			authorization_server_metadata_url: super::url_field(&options, "authorizationServerMetadataUrl")?,
			fetch: super::fetch(&options),
			signal: super::signal(&options),
			skip_issuer_validation: optional(&options, "skipIssuerValidation")?,
			skip_refresh: optional(&options, "skipRefresh")?,
		};
		Ok(super::promise(async move {
			pi_mcp::oauth::authorize_mcp(&*provider, options).await
		}))
	})
}

/// `adaptOAuthProvider(provider)`.
#[wasm_bindgen(js_name = mcpAdaptOAuthProvider)]
pub fn adapt_oauth_provider(
	native: Option<McpOAuthProviderHandle>,
	js: JsValue,
) -> Result<AuthProviderHandle, JsValue> {
	Ok(AuthProviderHandle::new(pi_mcp::oauth::adapt_oauth_provider(provider(
		native, js,
	)?)))
}

/// `startAuthorization(authorizationServerUrl, options)`, resolving to `{ authorizationUrl, codeVerifier }` with the
/// URL as a string.
#[wasm_bindgen(js_name = mcpStartAuthorization)]
pub fn start_authorization(authorization_server_url: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let url = super::url_string(&authorization_server_url).unwrap_or_default();
		let options = StartAuthorizationOptions {
			metadata: optional(&options, "metadata")?,
			client_information: crate::options::arg(field(&options, "clientInformation"))?,
			redirect_url: super::url_string(&field(&options, "redirectUrl")).unwrap_or_default(),
			scope: optional(&options, "scope")?,
			state: optional(&options, "state")?,
			resource: optional(&options, "resource")?,
		};
		Ok(super::promise(async move {
			let start = pi_mcp::oauth::start_authorization(&url, options).await?;
			Ok(serde_json::json!({
				"authorizationUrl": start.authorization_url.as_str(),
				"codeVerifier": start.code_verifier,
			}))
		}))
	})
}

/// `registerClient(authorizationServerUrl, options)`.
#[wasm_bindgen(js_name = mcpRegisterClient)]
pub fn register_client(authorization_server_url: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let url = super::url_string(&authorization_server_url).unwrap_or_default();
		let options = RegisterClientOptions {
			metadata: optional(&options, "metadata")?,
			client_metadata: crate::options::arg(field(&options, "clientMetadata"))?,
			scope: optional(&options, "scope")?,
			fetch: super::fetch(&options),
			signal: super::signal(&options),
		};
		Ok(super::promise(async move {
			pi_mcp::oauth::register_client(&url, options).await
		}))
	})
}

/// `exchangeAuthorizationCode(authorizationServerUrl, options)`.
#[wasm_bindgen(js_name = mcpExchangeAuthorizationCode)]
pub fn exchange_authorization_code(authorization_server_url: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let url = super::url_string(&authorization_server_url).unwrap_or_default();
		let code: String = crate::options::arg(field(&options, "code"))?;
		let code_verifier: String = crate::options::arg(field(&options, "codeVerifier"))?;
		let redirect_url = super::url_string(&field(&options, "redirectUrl")).unwrap_or_default();
		let request = token_request_options(&options)?;
		Ok(super::promise(async move {
			pi_mcp::oauth::exchange_authorization_code(&url, &request, &code, &code_verifier, &redirect_url).await
		}))
	})
}

/// `refreshAuthorization(authorizationServerUrl, options)`.
#[wasm_bindgen(js_name = mcpRefreshAuthorization)]
pub fn refresh_authorization(authorization_server_url: JsValue, options: JsValue) -> Promise {
	super::rejecting(|| {
		let url = super::url_string(&authorization_server_url).unwrap_or_default();
		let refresh_token: String = crate::options::arg(field(&options, "refreshToken"))?;
		let request = token_request_options(&options)?;
		Ok(super::promise(async move {
			pi_mcp::oauth::refresh_authorization(&url, &request, &refresh_token).await
		}))
	})
}

/// `stepUpScope(granted, challenged)`.
#[wasm_bindgen(js_name = mcpStepUpScope)]
pub fn step_up_scope(granted: Option<String>, challenged: Option<String>) -> Option<String> {
	pi_mcp::oauth::step_up_scope(granted.as_deref(), challenged.as_deref())
}

/// The provider a JS caller passed: the Rust provider of the facade's `McpOAuthProvider`, or the JS object.
fn provider(native: Option<McpOAuthProviderHandle>, js: JsValue) -> Result<Rc<dyn OAuthClientProvider>, JsValue> {
	match native {
		Some(handle) => Ok(handle.provider()),
		None => Ok(Rc::new(JsOAuthClientProvider::new(js)?)),
	}
}

/// `TokenRequestOptions`; `addClientAuthentication` is called as a method of `options`.
fn token_request_options(options: &JsValue) -> Result<TokenRequestOptions, JsValue> {
	Ok(TokenRequestOptions {
		metadata: optional(options, "metadata")?,
		client_information: crate::options::arg(field(options, "clientInformation"))?,
		resource: optional(options, "resource")?,
		add_client_authentication: function(options, "addClientAuthentication")
			.map(|add| add_client_authentication(add, options.clone())),
		fetch: super::fetch(options),
		signal: super::signal(options),
	})
}

/// `addClientAuthentication(headers, params, url, metadata)` over a JS function, which updates a `Headers` and a
/// `URLSearchParams` in place; header names come back lowercased, as `Headers` iterates them.
fn add_client_authentication(add: Function, this: JsValue) -> AddClientAuthentication {
	Rc::new(
		move |headers: OAuthHeaders, params: OAuthParams, url: Url, metadata: Option<AuthorizationServerMetadata>| {
			let (add, this) = (add.clone(), this.clone());
			Box::pin(async move {
				let headers = JsHeaders::new(&crate::json::to_js(&headers)?).map_err(crate::error::from_js)?;
				let pairs: Array = params
					.iter()
					.map(|(key, value)| Array::of2(&key.as_str().into(), &value.as_str().into()))
					.collect();
				let params = JsSearchParams::new(&pairs).map_err(crate::error::from_js)?;
				let metadata = match &metadata {
					Some(metadata) => crate::json::to_js(metadata)?,
					None => JsValue::UNDEFINED,
				};
				let args = [
					JsValue::from(&headers),
					JsValue::from(&params),
					url.as_str().into(),
					metadata,
				];
				crate::callback::call(&add, &this, &args).await?;
				Ok((entries(&headers), entries(&params)))
			}) as LocalBoxFuture<'static, PiResult<(OAuthHeaders, OAuthParams)>>
		},
	)
}

/// The `[name, value]` entries of an iterable `Headers` or `URLSearchParams`.
fn entries(iterable: &JsValue) -> indexmap::IndexMap<String, String> {
	let Ok(Some(iter)) = js_sys::try_iter(iterable) else {
		return indexmap::IndexMap::new();
	};
	iter.filter_map(Result::ok)
		.filter_map(|entry| {
			let entry: Array = entry.unchecked_into();
			Some((entry.get(0).as_string()?, entry.get(1).as_string()?))
		})
		.collect()
}

/// An `OAuthClientProvider` written in JS. `redirectUrl` and `clientMetadata` are read when it is wrapped; the
/// methods are looked up at call time and called as methods of the object.
pub struct JsOAuthClientProvider {
	object: JsValue,
	redirect_url: String,
	client_metadata: OAuthClientMetadata,
}

impl JsOAuthClientProvider {
	pub fn new(object: JsValue) -> Result<Self, JsValue> {
		Ok(Self {
			redirect_url: super::url_string(&field(&object, "redirectUrl")).unwrap_or_default(),
			client_metadata: crate::options::arg(field(&object, "clientMetadata"))?,
			object,
		})
	}

	/// `await object[method](...args)`; a missing method is the `TypeError` of calling it.
	fn call(&self, method: &'static str, args: Vec<JsValue>) -> LocalBoxFuture<'static, PiResult<JsValue>> {
		let function = function(&self.object, method);
		let object = self.object.clone();
		Box::pin(async move {
			let Some(function) = function else {
				return Err(PiError::type_error(format!("provider.{method} is not a function")));
			};
			crate::callback::call(&function, &object, &args).await
		})
	}

	/// `call` for an optional method: `undefined` when the object has none.
	fn call_optional(&self, method: &'static str, args: Vec<JsValue>) -> LocalBoxFuture<'static, PiResult<JsValue>> {
		if function(&self.object, method).is_none() {
			return Box::pin(async { Ok(JsValue::UNDEFINED) });
		}
		self.call(method, args)
	}

	/// `call` with one argument of plain data, ignoring the result.
	fn save<T: serde::Serialize>(
		&self,
		method: &'static str,
		value: &T,
		optional_method: bool,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let value = match crate::json::to_js(value) {
			Ok(value) => value,
			Err(error) => return Box::pin(async move { Err(error) }),
		};
		let call = if optional_method {
			self.call_optional(method, vec![value])
		} else {
			self.call(method, vec![value])
		};
		Box::pin(async move { call.await.map(|_| ()) })
	}

	/// `call` reading an optional result with serde.
	fn load<T: serde::de::DeserializeOwned + 'static>(
		&self,
		method: &'static str,
		optional_method: bool,
	) -> LocalBoxFuture<'static, PiResult<Option<T>>> {
		let call = if optional_method {
			self.call_optional(method, Vec::new())
		} else {
			self.call(method, Vec::new())
		};
		Box::pin(async move { super::optional_result(call.await?) })
	}
}

impl OAuthClientProvider for JsOAuthClientProvider {
	fn redirect_url(&self) -> &str {
		&self.redirect_url
	}

	fn client_metadata(&self) -> &OAuthClientMetadata {
		&self.client_metadata
	}

	fn client_metadata_document(
		&self,
		metadata: Option<&AuthorizationServerMetadata>,
	) -> Option<OAuthClientMetadataDocument> {
		let document = function(&self.object, "clientMetadataDocument")?;
		let metadata = match metadata {
			Some(metadata) => crate::json::to_js(metadata).ok()?,
			None => JsValue::UNDEFINED,
		};
		document_from_js(&document.call1(&self.object, &metadata).ok()?)
	}

	fn state(&self) -> LocalBoxFuture<'_, PiResult<Option<String>>> {
		let call = self.call_optional("state", Vec::new());
		Box::pin(async move { Ok(call.await?.as_string()) })
	}

	fn client_information(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthClientInformationMixed>>> {
		self.load("clientInformation", false)
	}

	fn can_save_client_information(&self) -> bool {
		function(&self.object, "saveClientInformation").is_some()
	}

	fn save_client_information(&self, information: OAuthClientInformationMixed) -> LocalBoxFuture<'_, PiResult<()>> {
		self.save("saveClientInformation", &information, true)
	}

	fn tokens(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthTokens>>> {
		self.load("tokens", false)
	}

	fn save_tokens(&self, tokens: OAuthTokens) -> LocalBoxFuture<'_, PiResult<()>> {
		self.save("saveTokens", &tokens, false)
	}

	fn redirect_to_authorization(&self, url: Url) -> LocalBoxFuture<'_, PiResult<()>> {
		let call = super::js_url(url.as_str()).map(|url| self.call("redirectToAuthorization", vec![url]));
		Box::pin(async move { call?.await.map(|_| ()) })
	}

	fn save_code_verifier(&self, verifier: String) -> LocalBoxFuture<'_, PiResult<()>> {
		self.save("saveCodeVerifier", &verifier, false)
	}

	fn code_verifier(&self) -> LocalBoxFuture<'_, PiResult<String>> {
		let call = self.call("codeVerifier", Vec::new());
		Box::pin(async move { Ok(call.await?.as_string().unwrap_or_default()) })
	}

	fn add_client_authentication(&self) -> Option<AddClientAuthentication> {
		function(&self.object, "addClientAuthentication").map(|add| add_client_authentication(add, self.object.clone()))
	}

	fn invalidate_credentials(&self, kind: CredentialKind) -> LocalBoxFuture<'_, PiResult<()>> {
		self.save("invalidateCredentials", &credential_kind_name(kind), true)
	}

	fn save_discovery_state(&self, state: OAuthDiscoveryState) -> LocalBoxFuture<'_, PiResult<()>> {
		self.save("saveDiscoveryState", &state, true)
	}

	fn discovery_state(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthDiscoveryState>>> {
		self.load("discoveryState", true)
	}
}

/// An `OAuthClientMetadataDocument` (`{ url, redirectUrl }`) from JS; `None` for `undefined`.
pub fn document_from_js(document: &JsValue) -> Option<OAuthClientMetadataDocument> {
	Some(OAuthClientMetadataDocument {
		url: field(document, "url").as_string()?,
		redirect_url: super::url_string(&field(document, "redirectUrl"))?,
	})
}

/// The JS value of an `OAuthClientMetadataDocument`.
pub fn document_to_js(document: Option<OAuthClientMetadataDocument>) -> Result<JsValue, JsValue> {
	super::defined(
		document.map(|document| serde_json::json!({ "url": document.url, "redirectUrl": document.redirect_url })),
	)
}

/// The `kind` argument of `invalidateCredentials`.
pub fn credential_kind_name(kind: CredentialKind) -> &'static str {
	match kind {
		CredentialKind::All => "all",
		CredentialKind::Client => "client",
		CredentialKind::Tokens => "tokens",
		CredentialKind::Verifier => "verifier",
		CredentialKind::Discovery => "discovery",
	}
}

/// The `CredentialKind` of an `invalidateCredentials` argument.
pub fn credential_kind(kind: &str) -> Result<CredentialKind, JsValue> {
	Ok(match kind {
		"all" => CredentialKind::All,
		"client" => CredentialKind::Client,
		"tokens" => CredentialKind::Tokens,
		"verifier" => CredentialKind::Verifier,
		"discovery" => CredentialKind::Discovery,
		_ => {
			return Err(crate::error::to_js(&PiError::type_error(format!(
				"Unknown OAuth credential kind {kind}"
			))));
		}
	})
}
