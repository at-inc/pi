//! Port of `packages/mcp/src/oauth/provider.ts`.
//! PORT-STATUS: reviewed
//!
//! `McpOAuthStateStore` is a trait. `MemoryOAuthStateStore` clones where TS uses `structuredClone`. The
//! options' `clientMetadata` is an `OAuthClientMetadata` whose empty `redirect_uris` stands for the absent TS
//! field, so an explicitly empty list also becomes `[redirectUrl]`. Writes are chained like the TS promise
//! chain and run to completion even when nobody awaits them, including its behavior after a failed save: every
//! later load and update fails with that error.

use std::cell::RefCell;
use std::rc::Rc;

use futures::FutureExt;
use futures::future::Shared;
use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde::{Deserialize, Serialize};
use url::Url;

use super::discovery::parse_url;
use super::flow::{CredentialKind, OAuthClientMetadataDocument, OAuthClientProvider, run_detached};
use super::types::{
	AuthorizationServerMetadata, OAuthClientInformation, OAuthClientInformationMixed, OAuthClientMetadata,
	OAuthDiscoveryState, OAuthTokens,
};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthState {
	pub server_url: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub client_information: Option<OAuthClientInformationMixed>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tokens: Option<OAuthTokens>,
	/// When the access token expires, in milliseconds since the epoch, from `expires_in` at the time it was saved.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tokens_expire_at: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub code_verifier: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub oauth_state: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub discovery: Option<OAuthDiscoveryState>,
}

pub trait McpOAuthStateStore {
	fn load(&self) -> LocalBoxFuture<'_, PiResult<Option<McpOAuthState>>>;
	fn save(&self, state: McpOAuthState) -> LocalBoxFuture<'_, PiResult<()>>;
}

/// `(metadata) => OAuthClientMetadataDocument | undefined`.
pub type ClientMetadataDocumentFunction =
	Rc<dyn Fn(Option<&AuthorizationServerMetadata>) -> Option<OAuthClientMetadataDocument>>;
/// `onRedirect(url)`.
pub type OnRedirectFunction = Rc<dyn Fn(Url) -> LocalBoxFuture<'static, PiResult<()>>>;

#[derive(Clone)]
pub struct McpOAuthProviderOptions {
	pub server_url: String,
	pub redirect_url: String,
	/// `redirect_uris` defaults to `[redirect_url]` when empty.
	pub client_metadata: OAuthClientMetadata,
	/// See `OAuthClientProvider::client_metadata_document`.
	pub client_metadata_document: Option<ClientMetadataDocumentFunction>,
	pub client_id: Option<String>,
	pub client_secret: Option<String>,
	pub store: Option<Rc<dyn McpOAuthStateStore>>,
	pub on_redirect: OnRedirectFunction,
}

#[derive(Default)]
pub struct MemoryOAuthStateStore {
	value: RefCell<Option<McpOAuthState>>,
}

impl MemoryOAuthStateStore {
	pub fn new() -> Self {
		Self::default()
	}
}

impl McpOAuthStateStore for MemoryOAuthStateStore {
	fn load(&self) -> LocalBoxFuture<'_, PiResult<Option<McpOAuthState>>> {
		let value = self.value.borrow().clone();
		Box::pin(async move { Ok(value) })
	}

	fn save(&self, state: McpOAuthState) -> LocalBoxFuture<'_, PiResult<()>> {
		*self.value.borrow_mut() = Some(state);
		Box::pin(async { Ok(()) })
	}
}

type Writes = Shared<LocalBoxFuture<'static, PiResult<()>>>;

/// Default stateful provider for one exact MCP server URL. Applications inject durable storage if needed.
pub struct McpOAuthProvider {
	redirect_url: String,
	client_metadata: OAuthClientMetadata,
	client_metadata_document: Option<ClientMetadataDocumentFunction>,
	server_url: String,
	configured_client: Option<OAuthClientInformationMixed>,
	store: Rc<dyn McpOAuthStateStore>,
	on_redirect: OnRedirectFunction,
	writes: RefCell<Writes>,
}

impl McpOAuthProvider {
	/// `new McpOAuthProvider(options)`. Fails like `new URL(serverUrl)` for an invalid server URL.
	pub fn new(options: McpOAuthProviderOptions) -> PiResult<Self> {
		let server_url = parse_url(&options.server_url)?.to_string();
		let nonempty = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
		let client_secret = nonempty(&options.client_secret);
		let metadata = options.client_metadata;
		let client_metadata = OAuthClientMetadata {
			redirect_uris: if metadata.redirect_uris.is_empty() {
				vec![options.redirect_url.clone()]
			} else {
				metadata.redirect_uris.clone()
			},
			grant_types: Some(
				metadata
					.grant_types
					.clone()
					.unwrap_or_else(|| vec!["authorization_code".into(), "refresh_token".into()]),
			),
			response_types: Some(metadata.response_types.clone().unwrap_or_else(|| vec!["code".into()])),
			token_endpoint_auth_method: Some(metadata.token_endpoint_auth_method.clone().unwrap_or_else(|| {
				if client_secret.is_some() {
					"client_secret_post"
				} else {
					"none"
				}
				.to_string()
			})),
			..metadata
		};
		let configured_client = nonempty(&options.client_id).map(|client_id| {
			OAuthClientInformationMixed::from(OAuthClientInformation {
				client_id,
				client_secret,
				..Default::default()
			})
		});
		Ok(Self {
			redirect_url: options.redirect_url,
			client_metadata,
			client_metadata_document: options.client_metadata_document,
			server_url,
			configured_client,
			store: options.store.unwrap_or_else(|| Rc::new(MemoryOAuthStateStore::new())),
			on_redirect: options.on_redirect,
			writes: RefCell::new(futures::future::ready(Ok(())).boxed_local().shared()),
		})
	}

	async fn load(&self) -> PiResult<McpOAuthState> {
		let writes = self.writes.borrow().clone();
		writes.await?;
		Ok(own(&self.server_url, self.store.load().await?))
	}

	async fn update(&self, update: impl FnOnce(McpOAuthState) -> McpOAuthState + 'static) -> PiResult<()> {
		let previous = self.writes.borrow().clone();
		let store = self.store.clone();
		let server_url = self.server_url.clone();
		let next = run_detached(async move {
			previous.await?;
			let state = own(&server_url, store.load().await?);
			store.save(update(state)).await
		});
		*self.writes.borrow_mut() = next.clone();
		next.await
	}
}

/// Stored state for another server URL is ignored so credentials never leak across servers.
fn own(server_url: &str, state: Option<McpOAuthState>) -> McpOAuthState {
	match state {
		Some(state) if state.server_url == server_url => state,
		_ => McpOAuthState {
			server_url: server_url.to_string(),
			..Default::default()
		},
	}
}

impl OAuthClientProvider for McpOAuthProvider {
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
		self.client_metadata_document.as_ref()?(metadata)
	}

	fn state(&self) -> LocalBoxFuture<'_, PiResult<Option<String>>> {
		Box::pin(async move {
			if let Some(existing) = self.load().await?.oauth_state.filter(|state| !state.is_empty()) {
				return Ok(Some(existing));
			}
			let state: String = rand::random::<[u8; 32]>()
				.iter()
				.map(|byte| format!("{byte:02x}"))
				.collect();
			let saved = state.clone();
			self.update(move |value| McpOAuthState {
				oauth_state: Some(saved),
				..value
			})
			.await?;
			Ok(Some(state))
		})
	}

	fn client_information(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthClientInformationMixed>>> {
		Box::pin(async move {
			if let Some(configured) = &self.configured_client {
				return Ok(Some(configured.clone()));
			}
			Ok(self.load().await?.client_information)
		})
	}

	fn can_save_client_information(&self) -> bool {
		true
	}

	fn save_client_information(&self, information: OAuthClientInformationMixed) -> LocalBoxFuture<'_, PiResult<()>> {
		Box::pin(async move {
			if self.configured_client.is_some() {
				return Ok(());
			}
			self.update(move |value| McpOAuthState {
				client_information: Some(information),
				..value
			})
			.await
		})
	}

	fn tokens(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthTokens>>> {
		Box::pin(async move { Ok(self.load().await?.tokens) })
	}

	fn save_tokens(&self, tokens: OAuthTokens) -> LocalBoxFuture<'_, PiResult<()>> {
		let expires_at = tokens
			.expires_in
			.map(|expires_in| pi_host::now_ms() + expires_in * 1000.0);
		Box::pin(async move {
			self.update(move |value| McpOAuthState {
				tokens: Some(tokens),
				tokens_expire_at: expires_at,
				..value
			})
			.await
		})
	}

	fn redirect_to_authorization(&self, url: Url) -> LocalBoxFuture<'_, PiResult<()>> {
		(self.on_redirect)(url)
	}

	fn save_code_verifier(&self, verifier: String) -> LocalBoxFuture<'_, PiResult<()>> {
		Box::pin(async move {
			self.update(move |value| McpOAuthState {
				code_verifier: Some(verifier),
				..value
			})
			.await
		})
	}

	fn code_verifier(&self) -> LocalBoxFuture<'_, PiResult<String>> {
		Box::pin(async move {
			match self.load().await?.code_verifier.filter(|verifier| !verifier.is_empty()) {
				Some(verifier) => Ok(verifier),
				None => Err(PiError::new("No OAuth PKCE code verifier is stored")),
			}
		})
	}

	fn invalidate_credentials(&self, kind: CredentialKind) -> LocalBoxFuture<'_, PiResult<()>> {
		Box::pin(async move {
			self.update(move |mut next| {
				let all = kind == CredentialKind::All;
				if all || kind == CredentialKind::Client {
					next.client_information = None;
				}
				if all || kind == CredentialKind::Tokens {
					next.tokens = None;
					next.tokens_expire_at = None;
				}
				if all || kind == CredentialKind::Verifier {
					next.code_verifier = None;
				}
				if all || kind == CredentialKind::Discovery {
					next.discovery = None;
				}
				if all {
					next.oauth_state = None;
				}
				next
			})
			.await
		})
	}

	fn save_discovery_state(&self, discovery: OAuthDiscoveryState) -> LocalBoxFuture<'_, PiResult<()>> {
		Box::pin(async move {
			self.update(move |value| McpOAuthState {
				discovery: Some(discovery),
				..value
			})
			.await
		})
	}

	fn discovery_state(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthDiscoveryState>>> {
		Box::pin(async move { Ok(self.load().await?.discovery) })
	}
}
