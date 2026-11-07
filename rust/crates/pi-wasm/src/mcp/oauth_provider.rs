//! Exports of `oauth/provider`: `MemoryOAuthStateStore` and `McpOAuthProvider` over the Rust ones. A store written
//! in JS (`{ load, save }`) backs a Rust provider through `JsStateStore`.

use std::rc::Rc;

use futures::FutureExt;
use js_sys::{Function, Promise};
use pi_host::{LocalBoxFuture, PiResult};
use pi_mcp::oauth::provider::ClientMetadataDocumentFunction;
use pi_mcp::oauth::{
	McpOAuthProvider, McpOAuthProviderOptions, McpOAuthState, McpOAuthStateStore, MemoryOAuthStateStore,
	OAuthClientProvider,
};
use serde_json::Value;
use url::Url;
use wasm_bindgen::prelude::*;

use super::{field, function, optional};

/// `new MemoryOAuthStateStore()`.
#[wasm_bindgen(js_name = mcpCreateMemoryOAuthStateStore)]
pub fn create_memory_oauth_state_store() -> MemoryOAuthStateStoreHandle {
	MemoryOAuthStateStoreHandle {
		inner: Rc::new(MemoryOAuthStateStore::new()),
	}
}

/// A Rust `MemoryOAuthStateStore`, owned by the facade's class.
#[wasm_bindgen]
pub struct MemoryOAuthStateStoreHandle {
	inner: Rc<MemoryOAuthStateStore>,
}

#[wasm_bindgen]
impl MemoryOAuthStateStoreHandle {
	/// Another handle to the same store, for passing by value.
	pub fn share(&self) -> MemoryOAuthStateStoreHandle {
		MemoryOAuthStateStoreHandle {
			inner: self.inner.clone(),
		}
	}

	/// `store.load()`, synchronous like the TS class: the memory store's future is ready at once.
	pub fn load(&self) -> Result<JsValue, JsValue> {
		let state = self.inner.load().now_or_never().transpose();
		super::defined(crate::options::throw(state)?.flatten())
	}

	/// `store.save(state)`, synchronous like `load`.
	pub fn save(&self, state: JsValue) -> Result<(), JsValue> {
		let state: McpOAuthState = crate::options::arg(state)?;
		crate::options::throw(self.inner.save(state).now_or_never().unwrap_or(Ok(())))
	}
}

/// `new McpOAuthProvider(options)`: `store_native` is the Rust store of the facade's `MemoryOAuthStateStore`,
/// `store_js` any other `options.store`.
#[wasm_bindgen(js_name = mcpCreateOAuthProvider)]
pub fn create_oauth_provider(
	options: JsValue,
	store_native: Option<MemoryOAuthStateStoreHandle>,
	store_js: JsValue,
) -> Result<McpOAuthProviderHandle, JsValue> {
	let store: Option<Rc<dyn McpOAuthStateStore>> = match store_native {
		Some(store) => Some(store.inner),
		None if store_js.is_object() => Some(Rc::new(JsStateStore { object: store_js })),
		None => None,
	};
	// `redirect_uris` defaults to `[redirectUrl]`, which the Rust options express as an empty list.
	let mut client_metadata: Value = crate::options::arg(field(&options, "clientMetadata"))?;
	if let Value::Object(metadata) = &mut client_metadata {
		metadata
			.entry("redirect_uris")
			.or_insert_with(|| Value::Array(Vec::new()));
	}
	let on_redirect = function(&options, "onRedirect");
	let this = options.clone();
	let provider = McpOAuthProvider::new(McpOAuthProviderOptions {
		server_url: super::url_string(&field(&options, "serverUrl")).unwrap_or_default(),
		redirect_url: super::url_string(&field(&options, "redirectUrl")).unwrap_or_default(),
		client_metadata: crate::options::throw(
			serde_json::from_value(client_metadata).map_err(pi_host::PiError::from),
		)?,
		client_metadata_document: function(&options, "clientMetadataDocument").map(|document| {
			let this = options.clone();
			Rc::new(move |metadata: Option<&pi_mcp::oauth::AuthorizationServerMetadata>| {
				let metadata = match metadata {
					Some(metadata) => crate::json::to_js(metadata).ok()?,
					None => JsValue::UNDEFINED,
				};
				super::oauth_flow::document_from_js(&document.call1(&this, &metadata).ok()?)
			}) as ClientMetadataDocumentFunction
		}),
		client_id: optional(&options, "clientId")?,
		client_secret: optional(&options, "clientSecret")?,
		store,
		on_redirect: Rc::new(move |url: Url| {
			let (on_redirect, this) = (on_redirect.clone(), this.clone());
			Box::pin(async move { call_on_redirect(on_redirect, this, url).await })
		}),
	});
	Ok(McpOAuthProviderHandle {
		inner: Rc::new(crate::options::throw(provider)?),
	})
}

/// `options.onRedirect(new URL(url))`.
async fn call_on_redirect(on_redirect: Option<Function>, this: JsValue, url: Url) -> PiResult<()> {
	let Some(on_redirect) = on_redirect else {
		return Err(pi_host::PiError::type_error("options.onRedirect is not a function"));
	};
	crate::callback::call(&on_redirect, &this, &[super::js_url(url.as_str())?]).await?;
	Ok(())
}

/// A Rust `McpOAuthProvider`, owned by the facade's class.
#[wasm_bindgen]
pub struct McpOAuthProviderHandle {
	inner: Rc<McpOAuthProvider>,
}

impl McpOAuthProviderHandle {
	/// The provider as an `OAuthClientProvider`.
	pub fn provider(&self) -> Rc<dyn OAuthClientProvider> {
		self.inner.clone()
	}
}

#[wasm_bindgen]
impl McpOAuthProviderHandle {
	/// Another handle to the same provider, for passing by value.
	pub fn share(&self) -> McpOAuthProviderHandle {
		McpOAuthProviderHandle {
			inner: self.inner.clone(),
		}
	}

	/// `provider.redirectUrl`.
	#[wasm_bindgen(js_name = redirectUrl)]
	pub fn redirect_url(&self) -> String {
		self.inner.redirect_url().into()
	}

	/// `provider.clientMetadata`.
	#[wasm_bindgen(js_name = clientMetadata)]
	pub fn client_metadata(&self) -> Result<JsValue, JsValue> {
		crate::options::throw(crate::json::to_js(self.inner.client_metadata()))
	}

	/// `provider.clientMetadataDocument(metadata)`.
	#[wasm_bindgen(js_name = clientMetadataDocument)]
	pub fn client_metadata_document(&self, metadata: JsValue) -> Result<JsValue, JsValue> {
		let metadata: Option<pi_mcp::oauth::AuthorizationServerMetadata> =
			super::optional_result(metadata).map_err(|error| crate::error::to_js(&error))?;
		super::oauth_flow::document_to_js(self.inner.client_metadata_document(metadata.as_ref()))
	}

	/// `provider.state()`.
	pub fn state(&self) -> Promise {
		let provider = self.inner.clone();
		super::optional_promise(async move { provider.state().await })
	}

	/// `provider.clientInformation()`.
	#[wasm_bindgen(js_name = clientInformation)]
	pub fn client_information(&self) -> Promise {
		let provider = self.inner.clone();
		super::optional_promise(async move { provider.client_information().await })
	}

	/// `provider.saveClientInformation(information)`.
	#[wasm_bindgen(js_name = saveClientInformation)]
	pub fn save_client_information(&self, information: JsValue) -> Promise {
		let provider = self.inner.clone();
		super::rejecting(|| {
			let information = crate::options::arg(information)?;
			Ok(super::unit_promise(async move {
				provider.save_client_information(information).await
			}))
		})
	}

	/// `provider.tokens()`.
	pub fn tokens(&self) -> Promise {
		let provider = self.inner.clone();
		super::optional_promise(async move { provider.tokens().await })
	}

	/// `provider.saveTokens(tokens)`.
	#[wasm_bindgen(js_name = saveTokens)]
	pub fn save_tokens(&self, tokens: JsValue) -> Promise {
		let provider = self.inner.clone();
		super::rejecting(|| {
			let tokens = crate::options::arg(tokens)?;
			Ok(super::unit_promise(async move { provider.save_tokens(tokens).await }))
		})
	}

	/// `provider.redirectToAuthorization(url)`.
	#[wasm_bindgen(js_name = redirectToAuthorization)]
	pub fn redirect_to_authorization(&self, url: JsValue) -> Promise {
		let provider = self.inner.clone();
		super::rejecting(|| {
			let url = super::url_arg(&url)?
				.ok_or_else(|| crate::error::to_js(&pi_host::PiError::type_error("Invalid URL: undefined")))?;
			Ok(super::unit_promise(async move {
				provider.redirect_to_authorization(url).await
			}))
		})
	}

	/// `provider.saveCodeVerifier(verifier)`.
	#[wasm_bindgen(js_name = saveCodeVerifier)]
	pub fn save_code_verifier(&self, verifier: String) -> Promise {
		let provider = self.inner.clone();
		super::unit_promise(async move { provider.save_code_verifier(verifier).await })
	}

	/// `provider.codeVerifier()`.
	#[wasm_bindgen(js_name = codeVerifier)]
	pub fn code_verifier(&self) -> Promise {
		let provider = self.inner.clone();
		super::promise(async move { provider.code_verifier().await })
	}

	/// `provider.invalidateCredentials(kind)`.
	#[wasm_bindgen(js_name = invalidateCredentials)]
	pub fn invalidate_credentials(&self, kind: &str) -> Promise {
		let provider = self.inner.clone();
		super::rejecting(|| {
			let kind = super::oauth_flow::credential_kind(kind)?;
			Ok(super::unit_promise(async move {
				provider.invalidate_credentials(kind).await
			}))
		})
	}

	/// `provider.saveDiscoveryState(state)`.
	#[wasm_bindgen(js_name = saveDiscoveryState)]
	pub fn save_discovery_state(&self, state: JsValue) -> Promise {
		let provider = self.inner.clone();
		super::rejecting(|| {
			let state = crate::options::arg(state)?;
			Ok(super::unit_promise(async move {
				provider.save_discovery_state(state).await
			}))
		})
	}

	/// `provider.discoveryState()`.
	#[wasm_bindgen(js_name = discoveryState)]
	pub fn discovery_state(&self) -> Promise {
		let provider = self.inner.clone();
		super::optional_promise(async move { provider.discovery_state().await })
	}
}

/// A `McpOAuthStateStore` written in JS, called as methods of the object.
struct JsStateStore {
	object: JsValue,
}

impl McpOAuthStateStore for JsStateStore {
	fn load(&self) -> LocalBoxFuture<'_, PiResult<Option<McpOAuthState>>> {
		let load = function(&self.object, "load");
		Box::pin(async move {
			let Some(load) = load else {
				return Err(pi_host::PiError::type_error("store.load is not a function"));
			};
			super::optional_result(crate::callback::call(&load, &self.object, &[]).await?)
		})
	}

	fn save(&self, state: McpOAuthState) -> LocalBoxFuture<'_, PiResult<()>> {
		let save = function(&self.object, "save");
		Box::pin(async move {
			let Some(save) = save else {
				return Err(pi_host::PiError::type_error("store.save is not a function"));
			};
			crate::callback::call(&save, &self.object, &[crate::json::to_js(&state)?]).await?;
			Ok(())
		})
	}
}
