//! Port of `packages/mcp/test/oauth.test.ts`.
//!
//! The `node:http` test servers are the in-process `FakeNetwork` of `tests/common`. Tests that use
//! `OAuthCallbackServer` need the `native` feature: they run on a tokio host whose `fetch` reaches the
//! in-process servers and the real callback socket.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{ServerRequest, ServerResponse, json, run, sleep, spawn};
use pi_host::{AbortController, HttpResponse, LocalBoxFuture, PiResult};
use pi_mcp::auth_provider::{McpFetch, UnauthorizedContext};
use pi_mcp::oauth::errors::McpOAuthAuthorizationRequiredError;
use pi_mcp::oauth::flow::CredentialKind;
use pi_mcp::oauth::flow::{RegisterClientOptions, StartAuthorizationOptions};
use pi_mcp::oauth::{
	AuthorizationServerMetadata, McpOAuthProvider, McpOAuthProviderOptions, McpOAuthStateStore, MemoryOAuthStateStore,
	OAuthClientInformation, OAuthClientInformationMixed, OAuthClientMetadata, OAuthClientProvider, OAuthDiscoveryState,
	OAuthFlowOptions, OAuthFlowResult, OAuthInsecureEndpointError, OAuthIssuerMismatchError, OAuthTokens,
	adapt_oauth_provider, authorize_mcp, discover_authorization_server_metadata, register_client, start_authorization,
};
use serde_json::{Value, json as value};
use url::Url;

fn ready<'a, T: 'a>(value: T) -> LocalBoxFuture<'a, T> {
	Box::pin(std::future::ready(value))
}

struct TestOAuthProvider {
	redirect_url: String,
	client_metadata: OAuthClientMetadata,
	client: RefCell<Option<OAuthClientInformationMixed>>,
	token_set: RefCell<Option<OAuthTokens>>,
	verifier: RefCell<Option<String>>,
	discovery: RefCell<Option<OAuthDiscoveryState>>,
	authorization_url: RefCell<Option<Url>>,
}

impl TestOAuthProvider {
	fn new(redirect_url: &str) -> Rc<Self> {
		Rc::new(Self {
			redirect_url: redirect_url.to_string(),
			client_metadata: OAuthClientMetadata {
				redirect_uris: vec![redirect_url.to_string()],
				client_name: Some("pi-mcp-test".into()),
				grant_types: Some(vec!["authorization_code".into(), "refresh_token".into()]),
				response_types: Some(vec!["code".into()]),
				token_endpoint_auth_method: Some("none".into()),
				..Default::default()
			},
			client: RefCell::new(None),
			token_set: RefCell::new(None),
			verifier: RefCell::new(None),
			discovery: RefCell::new(None),
			authorization_url: RefCell::new(None),
		})
	}

	fn with_client(redirect_url: &str) -> Rc<Self> {
		let provider = Self::new(redirect_url);
		*provider.client.borrow_mut() = Some(client_information("client"));
		provider
	}

	fn authorization_param(&self, name: &str) -> Option<String> {
		let url = self.authorization_url.borrow().clone()?;
		url.query_pairs()
			.find(|(key, _)| key == name)
			.map(|(_, value)| value.into_owned())
	}
}

impl OAuthClientProvider for TestOAuthProvider {
	fn redirect_url(&self) -> &str {
		&self.redirect_url
	}

	fn client_metadata(&self) -> &OAuthClientMetadata {
		&self.client_metadata
	}

	fn state(&self) -> LocalBoxFuture<'_, PiResult<Option<String>>> {
		ready(Ok(Some("expected-state".into())))
	}

	fn client_information(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthClientInformationMixed>>> {
		ready(Ok(self.client.borrow().clone()))
	}

	fn can_save_client_information(&self) -> bool {
		true
	}

	fn save_client_information(&self, information: OAuthClientInformationMixed) -> LocalBoxFuture<'_, PiResult<()>> {
		*self.client.borrow_mut() = Some(information);
		ready(Ok(()))
	}

	fn tokens(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthTokens>>> {
		ready(Ok(self.token_set.borrow().clone()))
	}

	fn save_tokens(&self, tokens: OAuthTokens) -> LocalBoxFuture<'_, PiResult<()>> {
		*self.token_set.borrow_mut() = Some(tokens);
		ready(Ok(()))
	}

	fn redirect_to_authorization(&self, url: Url) -> LocalBoxFuture<'_, PiResult<()>> {
		*self.authorization_url.borrow_mut() = Some(url);
		ready(Ok(()))
	}

	fn save_code_verifier(&self, verifier: String) -> LocalBoxFuture<'_, PiResult<()>> {
		*self.verifier.borrow_mut() = Some(verifier);
		ready(Ok(()))
	}

	fn code_verifier(&self) -> LocalBoxFuture<'_, PiResult<String>> {
		ready(
			self.verifier
				.borrow()
				.clone()
				.ok_or_else(|| pi_host::PiError::new("Missing code verifier")),
		)
	}

	fn invalidate_credentials(&self, kind: CredentialKind) -> LocalBoxFuture<'_, PiResult<()>> {
		let all = kind == CredentialKind::All;
		if all || kind == CredentialKind::Client {
			*self.client.borrow_mut() = None;
		}
		if all || kind == CredentialKind::Tokens {
			*self.token_set.borrow_mut() = None;
		}
		if all || kind == CredentialKind::Verifier {
			*self.verifier.borrow_mut() = None;
		}
		if all || kind == CredentialKind::Discovery {
			*self.discovery.borrow_mut() = None;
		}
		ready(Ok(()))
	}

	fn save_discovery_state(&self, state: OAuthDiscoveryState) -> LocalBoxFuture<'_, PiResult<()>> {
		*self.discovery.borrow_mut() = Some(state);
		ready(Ok(()))
	}

	fn discovery_state(&self) -> LocalBoxFuture<'_, PiResult<Option<OAuthDiscoveryState>>> {
		ready(Ok(self.discovery.borrow().clone()))
	}
}

fn client_information(client_id: &str) -> OAuthClientInformationMixed {
	OAuthClientInformation {
		client_id: client_id.into(),
		..Default::default()
	}
	.into()
}

fn tokens(access_token: &str, refresh_token: Option<&str>, scope: Option<&str>) -> OAuthTokens {
	OAuthTokens {
		access_token: access_token.into(),
		token_type: "Bearer".into(),
		refresh_token: refresh_token.map(str::to_string),
		scope: scope.map(str::to_string),
		..Default::default()
	}
}

/// The request's origin: `serverOrigin` of the TS handlers.
fn origin_of(request: &ServerRequest) -> String {
	request.url.origin().ascii_serialization()
}

fn respond_json(response: &ServerResponse, body: Value) {
	response.set_header("content-type", "application/json");
	response.end(&pi_host::js::json_stringify(&body));
}

fn not_found(response: &ServerResponse) {
	response.set_status(404);
	response.end("");
}

fn host_fetch() -> McpFetch {
	Rc::new(|request| Box::pin(async move { pi_host::fetch(request, None).await }))
}

fn form(body: &str) -> Vec<(String, String)> {
	url::form_urlencoded::parse(body.as_bytes()).into_owned().collect()
}

fn form_get(params: &[(String, String)], name: &str) -> Option<String> {
	params
		.iter()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.clone())
}

fn unauthorized_response(status: u16, www_authenticate: &str) -> HttpResponse {
	HttpResponse {
		status,
		status_text: String::new(),
		url: String::new(),
		headers: [("www-authenticate".to_string(), www_authenticate.to_string())]
			.into_iter()
			.collect(),
		body: Box::pin(futures::stream::empty()),
	}
}

fn metadata(origin: &str) -> AuthorizationServerMetadata {
	AuthorizationServerMetadata {
		issuer: origin.into(),
		authorization_endpoint: format!("{origin}/authorize"),
		token_endpoint: format!("{origin}/token"),
		response_types_supported: vec!["code".into()],
		..Default::default()
	}
}

fn flow(server_url: &str) -> OAuthFlowOptions {
	OAuthFlowOptions {
		server_url: server_url.into(),
		..Default::default()
	}
}

#[cfg(feature = "native")]
mod native {
	use std::cell::Cell;

	use base64::Engine;
	use pi_host::HttpRequest;
	use pi_mcp::client::{McpClient, McpClientOptions, McpRequestOptions};
	use pi_mcp::oauth::{OAuthCallbackPage, OAuthCallbackServer, OAuthCallbackServerOptions};
	use pi_mcp::transports::streamable_http::{StreamableHttpTransport, StreamableHttpTransportOptions};
	use sha2::{Digest, Sha256};

	use super::common::run_native;
	use super::*;

	fn get(url: &str) -> HttpRequest {
		HttpRequest {
			method: "GET".into(),
			url: url.into(),
			headers: Default::default(),
			body: None,
			signal: None,
		}
	}

	async fn fetch(url: &str) -> HttpResponse {
		pi_host::fetch(get(url), None).await.unwrap()
	}

	fn oauth_client() -> McpClient {
		McpClient::new(McpClientOptions {
			name: "oauth-test".into(),
			version: "1.0.0".into(),
			..Default::default()
		})
	}

	fn transport(origin: &str, provider: &Rc<TestOAuthProvider>) -> Rc<StreamableHttpTransport> {
		Rc::new(
			StreamableHttpTransport::new(StreamableHttpTransportOptions {
				url: format!("{origin}/mcp"),
				headers: Some(
					[(
						"Authorization".to_string(),
						"Bearer caller-supplied-stale-token".to_string(),
					)]
					.into_iter()
					.collect(),
				),
				auth_provider: Some(adapt_oauth_provider(provider.clone())),
				open_get_stream: Some(false),
				..Default::default()
			})
			.unwrap(),
		)
	}

	fn handle(
		request: ServerRequest,
		response: ServerResponse,
		expected_challenge: &RefCell<Option<String>>,
		refreshes: &Cell<u32>,
	) {
		let server_origin = origin_of(&request);
		match request.path() {
			"/.well-known/oauth-protected-resource/mcp" => respond_json(
				&response,
				value!({
					"resource": format!("{server_origin}/mcp"),
					"authorization_servers": [server_origin],
					"scopes_supported": ["org:read"],
				}),
			),
			"/.well-known/oauth-authorization-server" => respond_json(
				&response,
				value!({
					"issuer": server_origin,
					"authorization_endpoint": format!("{server_origin}/authorize"),
					"token_endpoint": format!("{server_origin}/token"),
					"registration_endpoint": format!("{server_origin}/register"),
					"response_types_supported": ["code"],
					"grant_types_supported": ["authorization_code", "refresh_token"],
					"token_endpoint_auth_methods_supported": ["none"],
					"code_challenge_methods_supported": ["S256"],
				}),
			),
			"/register" => {
				let mut metadata = request.json();
				metadata["client_id"] = value!("test-client");
				// Empty and null optional fields count as absent (#10266).
				metadata["client_secret"] = value!("");
				response.write_head(201, &[("content-type", "application/json")]);
				response.end(&metadata.to_string());
			}
			"/authorize" => {
				let param = |name: &str| {
					request
						.url
						.query_pairs()
						.find(|(key, _)| key == name)
						.map(|(_, value)| value.into_owned())
				};
				*expected_challenge.borrow_mut() = param("code_challenge");
				let mut redirect = Url::parse(&param("redirect_uri").unwrap_or_default()).unwrap();
				redirect
					.query_pairs_mut()
					.append_pair("code", "test-code")
					.append_pair("state", &param("state").unwrap_or_default());
				response.write_head(302, &[("location", redirect.as_str())]).end("");
			}
			"/token" => {
				let params = form(&request.body);
				if form_get(&params, "grant_type").as_deref() == Some("refresh_token") {
					refreshes.set(refreshes.get() + 1);
					respond_json(
						&response,
						value!({
							"access_token": "refreshed-token",
							"token_type": "Bearer",
							"refresh_token": "",
							"expires_in": null,
						}),
					);
					return;
				}
				let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
					.encode(Sha256::digest(form_get(&params, "code_verifier").unwrap_or_default()));
				if form_get(&params, "code").as_deref() != Some("test-code")
					|| Some(challenge) != *expected_challenge.borrow()
				{
					response.write_head(400, &[("content-type", "application/json")]);
					response.end(&value!({ "error": "invalid_grant" }).to_string());
					return;
				}
				respond_json(
					&response,
					value!({
						"access_token": "first-token",
						"refresh_token": "refresh-token",
						"token_type": "Bearer",
						"scope": "",
					}),
				);
			}
			"/mcp" => match request.method.as_str() {
				"GET" => {
					response.set_status(405);
					response.end("");
				}
				"DELETE" => {
					response.set_status(200);
					response.end("");
				}
				_ => {
					let token = request.header("authorization");
					if token != Some("Bearer first-token") && token != Some("Bearer refreshed-token") {
						let challenge = format!(
							"Bearer resource_metadata=\"{server_origin}/.well-known/oauth-protected-resource/mcp\", scope=\"\""
						);
						// An empty scope falls through to the resource metadata's scopes_supported.
						response.write_head(401, &[("www-authenticate", challenge.as_str())]);
						response.end("Unauthorized");
						return;
					}
					let message = request.json();
					let Some(id) = message.get("id") else {
						response.set_status(202);
						response.end("");
						return;
					};
					let result = if message["method"] == "initialize" {
						value!({
							"protocolVersion": "2025-06-18",
							"capabilities": { "tools": {} },
							"serverInfo": { "name": "oauth-test", "version": "1.0.0" },
						})
					} else {
						value!({ "tools": [{ "name": "issues", "inputSchema": { "type": "object" } }] })
					};
					respond_json(&response, value!({ "jsonrpc": "2.0", "id": id, "result": result }));
				}
			},
			_ => not_found(&response),
		}
	}

	#[test]
	fn discovers_registers_authorizes_with_pkce_and_refreshes_on_401() {
		run_native(|host| async move {
			let expected_challenge = Rc::new(RefCell::new(None::<String>));
			let refreshes = Rc::new(Cell::new(0));
			let (challenge, count) = (expected_challenge.clone(), refreshes.clone());
			let origin = host.listen(move |request, response| {
				handle(request, response, &challenge, &count);
				Box::pin(async { Ok(()) })
			});

			let callback = OAuthCallbackServer::listen(OAuthCallbackServerOptions::default())
				.await
				.unwrap();
			let provider = TestOAuthProvider::new(callback.redirect_url());
			let first_client = oauth_client();
			let error = first_client.connect(transport(&origin, &provider)).await.unwrap_err();
			assert!(
				McpOAuthAuthorizationRequiredError::is(&error),
				"{}: {}",
				error.name,
				error.message
			);
			assert_eq!(provider.authorization_param("scope").as_deref(), Some("org:read"));
			assert_eq!(provider.authorization_param("resource"), Some(format!("{origin}/mcp")));

			let callback_result = callback.wait_for_callback("expected-state", None).unwrap();
			let authorization_url = provider.authorization_url.borrow().clone().unwrap();
			let authorization_response = fetch(authorization_url.as_str()).await;
			let location = authorization_response.header("location").unwrap().to_string();
			drop(fetch(&location).await);
			let code = callback_result.await.unwrap().code;
			assert_eq!(
				authorize_mcp(
					&*provider,
					OAuthFlowOptions {
						authorization_code: Some(code),
						fetch: Some(host_fetch()),
						..flow(&format!("{origin}/mcp"))
					},
				)
				.await
				.unwrap(),
				OAuthFlowResult::Authorized
			);

			let client = oauth_client();
			client.connect(transport(&origin, &provider)).await.unwrap();
			assert_eq!(
				serde_json::to_value(client.list_tools(McpRequestOptions::default()).await.unwrap()).unwrap(),
				value!([{ "name": "issues", "inputSchema": { "type": "object" } }])
			);
			client.close().await.unwrap();

			let stale = OAuthTokens {
				access_token: "stale-token".into(),
				..provider.token_set.borrow().clone().unwrap()
			};
			*provider.token_set.borrow_mut() = Some(stale);
			let refreshed_client = oauth_client();
			refreshed_client.connect(transport(&origin, &provider)).await.unwrap();
			// Neither token response names a scope, so the grant has the requested scope.
			assert_eq!(
				*provider.token_set.borrow(),
				Some(tokens("refreshed-token", Some("refresh-token"), Some("org:read")))
			);
			assert_eq!(refreshes.get(), 1);
			refreshed_client.close().await.unwrap();
			callback.close().await.unwrap();
		});
	}

	#[test]
	fn renders_plain_text_by_default() {
		run_native(|_| async {
			let callback = OAuthCallbackServer::listen(OAuthCallbackServerOptions::default())
				.await
				.unwrap();
			let pending = callback.wait_for_callback("s1", None).unwrap();
			let response = fetch(&format!("{}?code=abc&state=s1", callback.redirect_url())).await;
			assert_eq!(response.header("content-type"), Some("text/plain; charset=utf-8"));
			assert_eq!(
				response.text().await.unwrap(),
				"Authorization complete. You may close this window."
			);
			assert_eq!(pending.await.unwrap().code, "abc");
			callback.close().await.unwrap();
		});
	}

	// #10302
	#[test]
	fn rejects_a_response_on_another_path_than_the_expected_one() {
		run_native(|_| async {
			let callback = OAuthCallbackServer::listen(OAuthCallbackServerOptions {
				extra_paths: Some(vec!["/callback/server-id".into()]),
				..Default::default()
			})
			.await
			.unwrap();
			let origin = Url::parse(callback.redirect_url())
				.unwrap()
				.origin()
				.ascii_serialization();
			let mixed_up = callback.wait_for_callback("s1", Some("/callback/server-id")).unwrap();
			let wrong = fetch(&format!("{origin}/callback?code=abc&state=s1")).await;
			assert_eq!(wrong.status, 400);
			let error = mixed_up.await.unwrap_err();
			assert!(
				error.message.contains("arrived on another redirect URI"),
				"{}",
				error.message
			);

			let pending = callback.wait_for_callback("s2", Some("/callback/server-id")).unwrap();
			let right = fetch(&format!("{origin}/callback/server-id?code=abc&state=s2")).await;
			assert_eq!(right.status, 200);
			assert_eq!(pending.await.unwrap().code, "abc");
			callback.close().await.unwrap();
		});
	}

	#[test]
	fn renders_pages_through_render_page() {
		run_native(|_| async {
			let pages = Rc::new(RefCell::new(Vec::<OAuthCallbackPage>::new()));
			let seen = pages.clone();
			let callback = OAuthCallbackServer::listen(OAuthCallbackServerOptions {
				render_page: Some(Rc::new(move |page| {
					seen.borrow_mut().push(page.clone());
					match page {
						OAuthCallbackPage::Ok => "<p>ok</p>".into(),
						OAuthCallbackPage::Failed { message, .. } => format!("<p>{message}</p>"),
					}
				})),
				..Default::default()
			})
			.await
			.unwrap();
			let denied = callback.wait_for_callback("s1", None).unwrap();
			let failure = fetch(&format!(
				"{}?error=access_denied&error_description=Denied&state=s1",
				callback.redirect_url()
			))
			.await;
			assert_eq!(failure.header("content-type"), Some("text/html; charset=utf-8"));
			let error = denied.await.unwrap_err();
			assert!(error.message.contains("Denied"), "{}", error.message);
			assert_eq!(
				pages.borrow().last(),
				Some(&OAuthCallbackPage::Failed {
					message: "Authorization failed. You may close this window.".into(),
					details: Some("Denied".into()),
				})
			);

			let pending = callback.wait_for_callback("s2", None).unwrap();
			let success = fetch(&format!("{}?code=abc&state=s2", callback.redirect_url())).await;
			assert_eq!(success.text().await.unwrap(), "<p>ok</p>");
			assert_eq!(pending.await.unwrap().code, "abc");
			callback.close().await.unwrap();
		});
	}
}

mod mcp_oauth {
	use super::*;

	#[test]
	fn shares_one_refresh_between_concurrent_401s_when_refresh_tokens_rotate() {
		run(|host| async move {
			let grants = Rc::new(RefCell::new(Vec::<String>::new()));
			let seen = grants.clone();
			let origin = host.listen(move |request, response| {
				let seen = seen.clone();
				Box::pin(async move {
					let server_origin = origin_of(&request);
					match request.path() {
						"/.well-known/oauth-protected-resource/mcp" => {
							// Invalid resource metadata falls back to the server origin instead of failing discovery.
							respond_json(
								&response,
								value!({ "resource": format!("{server_origin}/mcp"), "authorization_servers": ["not a url"] }),
							);
						}
						"/.well-known/oauth-authorization-server" => respond_json(
							&response,
							// Issuer without the trailing slash that URL parsing adds to the fallback server URL.
							value!({
								"issuer": server_origin,
								"authorization_endpoint": format!("{server_origin}/authorize"),
								"token_endpoint": format!("{server_origin}/token"),
								"response_types_supported": ["code"],
							}),
						),
						"/token" => {
							let refresh_token = form_get(&form(&request.body), "refresh_token").unwrap_or_default();
							seen.borrow_mut().push(refresh_token.clone());
							response.set_header("content-type", "application/json");
							if refresh_token != "r1" {
								response.set_status(400);
								response.end(&value!({ "error": "invalid_grant" }).to_string());
								return Ok(());
							}
							sleep(20.0).await;
							response.end(
								&value!({ "access_token": "a2", "refresh_token": "r2", "token_type": "Bearer", "expires_in": 3600 })
									.to_string(),
							);
						}
						_ => not_found(&response),
					}
					Ok(())
				})
			});
			let store = Rc::new(MemoryOAuthStateStore::new());
			let provider = Rc::new(
				McpOAuthProvider::new(McpOAuthProviderOptions {
					server_url: format!("{origin}/mcp"),
					redirect_url: "http://127.0.0.1/callback".into(),
					client_metadata: OAuthClientMetadata {
						client_name: Some("test".into()),
						..Default::default()
					},
					client_metadata_document: None,
					client_id: Some("client".into()),
					client_secret: None,
					store: Some(store.clone()),
					on_redirect: Rc::new(|_| ready(Ok(()))),
				})
				.unwrap(),
			);
			provider.save_tokens(tokens("a1", Some("r1"), None)).await.unwrap();
			let auth = adapt_oauth_provider(provider);
			let on_unauthorized = auth.on_unauthorized.clone().unwrap();
			let unauthorized = || UnauthorizedContext {
				response: unauthorized_response(401, "Bearer"),
				server_url: Url::parse(&format!("{origin}/mcp")).unwrap(),
				fetch: host_fetch(),
				token: Some("a1".into()),
			};
			let (first, second) =
				futures::future::join(on_unauthorized(unauthorized()), on_unauthorized(unauthorized())).await;
			first.unwrap();
			second.unwrap();
			// A late 401 for a request that still carried the old token must not refresh again.
			on_unauthorized(unauthorized()).await.unwrap();
			assert_eq!(*grants.borrow(), vec!["r1".to_string()]);
			assert_eq!((auth.token)().await.unwrap().as_deref(), Some("a2"));
			let state = store.load().await.unwrap().unwrap();
			assert_eq!(state.tokens.unwrap().refresh_token.as_deref(), Some("r2"));
			assert!(state.tokens_expire_at.unwrap() > host.now() + 3_500_000.0);
		});
	}

	#[test]
	fn asks_for_authorization_instead_of_refreshing_when_the_server_needs_more_scope() {
		run(|host| async move {
			let provider = TestOAuthProvider::with_client("http://127.0.0.1/callback");
			*provider.token_set.borrow_mut() = Some(tokens("a1", Some("r1"), Some("repo read:org")));
			let origin = host.listen(|request, response| {
				Box::pin(async move {
					let server_origin = origin_of(&request);
					if request.path() == "/.well-known/oauth-authorization-server" {
						respond_json(&response, serde_json::to_value(metadata(&server_origin)).unwrap());
						return Ok(());
					}
					response.set_status(if request.path() == "/token" { 500 } else { 404 });
					response.end("");
					Ok(())
				})
			});
			let auth = adapt_oauth_provider(provider.clone());
			let error = (auth.on_unauthorized.clone().unwrap())(UnauthorizedContext {
				response: unauthorized_response(403, "Bearer error=\"insufficient_scope\", scope=\"repo admin\""),
				server_url: Url::parse(&format!("{origin}/mcp")).unwrap(),
				fetch: host_fetch(),
				token: Some("a1".into()),
			})
			.await
			.unwrap_err();
			assert!(
				McpOAuthAuthorizationRequiredError::is(&error),
				"{}: {}",
				error.name,
				error.message
			);
			// The challenge may list only the missing scopes; the new grant keeps the old ones too.
			assert_eq!(
				provider.authorization_param("scope").as_deref(),
				Some("repo read:org admin")
			);
			// The working grant is kept until the user authorizes the new scope.
			assert_eq!(provider.token_set.borrow().as_ref().unwrap().access_token, "a1");
		});
	}

	#[test]
	fn binds_persisted_credentials_to_the_exact_mcp_server_url() {
		run(|_| async {
			let store: Rc<dyn McpOAuthStateStore> = Rc::new(MemoryOAuthStateStore::new());
			let provider = |server_url: &str| {
				McpOAuthProvider::new(McpOAuthProviderOptions {
					server_url: server_url.into(),
					redirect_url: "http://127.0.0.1/callback".into(),
					client_metadata: OAuthClientMetadata {
						client_name: Some("test".into()),
						..Default::default()
					},
					client_metadata_document: None,
					client_id: None,
					client_secret: None,
					store: Some(store.clone()),
					on_redirect: Rc::new(|_| ready(Ok(()))),
				})
				.unwrap()
			};
			let first = provider("https://one.example/mcp");
			first.save_tokens(tokens("secret", None, None)).await.unwrap();
			assert_eq!(first.tokens().await.unwrap().unwrap().access_token, "secret");

			let second = provider("https://two.example/mcp");
			assert_eq!(second.tokens().await.unwrap(), None);
		});
	}

	// #10493
	#[test]
	fn registers_with_an_application_type_derived_from_the_redirect_uris_unless_one_is_set() {
		run(|host| async move {
			let bodies = Rc::new(RefCell::new(Vec::<Value>::new()));
			let seen = bodies.clone();
			let origin = host.listen(move |request, response| {
				let mut metadata = request.json();
				seen.borrow_mut().push(metadata.clone());
				metadata["client_id"] = value!("client");
				response.write_head(201, &[("content-type", "application/json")]);
				response.end(&metadata.to_string());
				Box::pin(async { Ok(()) })
			});
			let register = |redirect_uris: &[&str], application_type: Option<&str>| {
				register_client(
					&origin,
					RegisterClientOptions {
						client_metadata: OAuthClientMetadata {
							redirect_uris: redirect_uris.iter().map(|uri| uri.to_string()).collect(),
							application_type: application_type.map(str::to_string),
							..Default::default()
						},
						..Default::default()
					},
				)
			};
			register(&["http://127.0.0.1:1234/callback"], None).await.unwrap();
			register(&["http://[::1]/callback"], None).await.unwrap();
			register(&["com.example.app:/callback"], None).await.unwrap();
			register(&["https://app.example/callback"], None).await.unwrap();
			register(&["http://localhost/callback"], Some("web")).await.unwrap();
			let types: Vec<Value> = bodies
				.borrow()
				.iter()
				.map(|body| body["application_type"].clone())
				.collect();
			assert_eq!(
				types,
				vec![
					json("\"native\""),
					json("\"native\""),
					json("\"native\""),
					json("\"web\""),
					json("\"web\"")
				]
			);
		});
	}

	#[test]
	fn rejects_authorization_metadata_whose_issuer_does_not_match_discovery() {
		run(|host| async move {
			let origin = host.listen(|request, response| {
				Box::pin(async move {
					let server_origin = origin_of(&request);
					if request.path() == "/.well-known/oauth-authorization-server" {
						respond_json(
							&response,
							serde_json::to_value(AuthorizationServerMetadata {
								issuer: "https://attacker.example".into(),
								..metadata(&server_origin)
							})
							.unwrap(),
						);
						return Ok(());
					}
					not_found(&response);
					Ok(())
				})
			});
			let error = discover_authorization_server_metadata(&origin, Default::default())
				.await
				.unwrap_err();
			assert!(
				OAuthIssuerMismatchError::is(&error),
				"{}: {}",
				error.name,
				error.message
			);
		});
	}

	#[test]
	fn rejects_non_http_authorization_endpoints_which_the_platform_url_handler_would_launch() {
		run(|host| async move {
			let endpoints = [
				"file://attacker.example/share/x.exe",
				"search-ms:query=x",
				"ms-msdt:/id",
			];
			let endpoint = Rc::new(RefCell::new(String::new()));
			let current = endpoint.clone();
			let origin = host.listen(move |request, response| {
				let server_origin = origin_of(&request);
				respond_json(
					&response,
					value!({
						"issuer": server_origin,
						"authorization_endpoint": *current.borrow(),
						"token_endpoint": format!("{server_origin}/token"),
						"response_types_supported": ["code"],
					}),
				);
				Box::pin(async { Ok(()) })
			});
			for value in endpoints {
				*endpoint.borrow_mut() = value.to_string();
				let error = discover_authorization_server_metadata(&origin, Default::default())
					.await
					.unwrap_err();
				assert!(
					error.message.contains("Invalid authorization endpoint"),
					"{}",
					error.message
				);
				let error = start_authorization(
					&origin,
					StartAuthorizationOptions {
						metadata: Some(AuthorizationServerMetadata {
							authorization_endpoint: value.into(),
							..metadata(&origin)
						}),
						client_information: client_information("client"),
						redirect_url: "http://127.0.0.1/callback".into(),
						scope: None,
						state: None,
						resource: None,
					},
				)
				.await
				.unwrap_err();
				assert!(
					error.message.contains("Invalid authorization endpoint"),
					"{}",
					error.message
				);
			}
		});
	}

	// #10172
	#[test]
	fn uses_a_configured_authorization_server_metadata_document_as_is() {
		run(|host| async move {
			let origin = host.listen(|request, response| {
				let server_origin = origin_of(&request);
				response.set_header("content-type", "application/json");
				match request.path() {
					// Names the MCP server itself, which serves no authorization server metadata.
					"/.well-known/oauth-protected-resource/mcp" => response.end(
						&value!({ "resource": format!("{server_origin}/mcp"), "authorization_servers": [server_origin] })
							.to_string(),
					),
					"/idp/metadata.json" => response.end(
						// Not derivable from the document URL; a configured document is not checked.
						&value!({
							"issuer": "https://idp.example",
							"authorization_endpoint": format!("{server_origin}/idp/authorize"),
							"token_endpoint": format!("{server_origin}/idp/token"),
							"response_types_supported": ["code"],
						})
						.to_string(),
					),
					_ => not_found(&response),
				}
				Box::pin(async { Ok(()) })
			});
			let provider = TestOAuthProvider::with_client("http://127.0.0.1/callback");
			let options = OAuthFlowOptions {
				authorization_server_metadata_url: Some(Url::parse(&format!("{origin}/idp/metadata.json")).unwrap()),
				..flow(&format!("{origin}/mcp"))
			};
			assert_eq!(
				authorize_mcp(&*provider, options.clone()).await.unwrap(),
				OAuthFlowResult::Redirect
			);
			let authorization_url = provider.authorization_url.borrow().clone().unwrap();
			assert_eq!(
				format!(
					"{}{}",
					authorization_url.origin().ascii_serialization(),
					authorization_url.path()
				),
				format!("{origin}/idp/authorize")
			);
			assert_eq!(provider.authorization_param("resource"), Some(format!("{origin}/mcp")));

			let insecure = OAuthFlowOptions {
				authorization_server_metadata_url: Some(Url::parse("http://idp.example/metadata.json").unwrap()),
				..options
			};
			let error = authorize_mcp(&*provider, insecure).await.unwrap_err();
			assert!(
				OAuthInsecureEndpointError::is(&error),
				"{}: {}",
				error.name,
				error.message
			);
		});
	}

	async fn exchange(
		origin: &str,
		code: &str,
		iss: Option<&str>,
		iss_parameter_supported: bool,
		with_metadata: bool,
	) -> PiResult<OAuthFlowResult> {
		let provider = TestOAuthProvider::with_client("http://127.0.0.1/callback");
		*provider.verifier.borrow_mut() = Some("verifier".into());
		*provider.discovery.borrow_mut() = Some(OAuthDiscoveryState {
			authorization_server_url: origin.into(),
			authorization_server_metadata: with_metadata.then(|| AuthorizationServerMetadata {
				authorization_response_iss_parameter_supported: Some(iss_parameter_supported),
				..metadata(origin)
			}),
			..Default::default()
		});
		authorize_mcp(
			&*provider,
			OAuthFlowOptions {
				authorization_code: Some(code.into()),
				iss: iss.map(str::to_string),
				..flow(&format!("{origin}/mcp"))
			},
		)
		.await
	}

	#[test]
	fn exchanges_a_code_only_when_its_iss_parameter_names_the_authorization_server() {
		run(|host| async move {
			let codes = Rc::new(RefCell::new(Vec::<String>::new()));
			let seen = codes.clone();
			let origin = host.listen(move |request, response| {
				if request.method != "POST" {
					not_found(&response);
					return Box::pin(async { Ok(()) });
				}
				seen.borrow_mut()
					.push(form_get(&form(&request.body), "code").unwrap_or_default());
				respond_json(&response, value!({ "access_token": "token", "token_type": "Bearer" }));
				Box::pin(async { Ok(()) })
			});
			let mismatch = |result: PiResult<OAuthFlowResult>| {
				let error = result.unwrap_err();
				assert!(
					OAuthIssuerMismatchError::is(&error),
					"{}: {}",
					error.name,
					error.message
				);
			};
			mismatch(exchange(&origin, "other", Some("https://attacker.example"), false, true).await);
			mismatch(exchange(&origin, "missing", None, true, true).await);
			assert_eq!(
				exchange(&origin, "matching", Some(&origin), true, true).await.unwrap(),
				OAuthFlowResult::Authorized
			);
			// Servers that do not promise the parameter may omit it.
			assert_eq!(
				exchange(&origin, "omitted", None, false, true).await.unwrap(),
				OAuthFlowResult::Authorized
			);
			// Without metadata there is no issuer to compare with, so a server cannot skip the check by hiding it.
			mismatch(exchange(&origin, "unverifiable", Some("https://attacker.example"), false, false).await);
			assert_eq!(
				exchange(&origin, "no-metadata", None, false, false).await.unwrap(),
				OAuthFlowResult::Authorized
			);
			assert_eq!(
				*codes.borrow(),
				vec!["matching".to_string(), "omitted".into(), "no-metadata".into()]
			);
		});
	}

	async fn run_until_stalled(
		origin: &str,
		stalled: &Rc<RefCell<Vec<String>>>,
		provider: Rc<TestOAuthProvider>,
	) -> Option<String> {
		let controller = AbortController::new();
		let count = stalled.borrow().len();
		let flow_provider = provider.clone();
		let options = OAuthFlowOptions {
			signal: Some(controller.signal()),
			..flow(&format!("{origin}/mcp"))
		};
		let settled = spawn(async move { authorize_mcp(&*flow_provider, options).await });
		for _ in 0..1_000 {
			if stalled.borrow().len() == count + 1 {
				break;
			}
			sleep(1.0).await;
		}
		assert_eq!(stalled.borrow().len(), count + 1);
		controller.abort();
		assert_eq!(settled.await.unwrap_err().name, "AbortError");
		assert!(provider.authorization_url.borrow().is_none());
		stalled.borrow().last().cloned()
	}

	// #10565
	#[test]
	fn stops_when_its_signal_aborts_without_falling_back_to_a_redirect() {
		run(|host| async move {
			let stalled = Rc::new(RefCell::new(Vec::<String>::new()));
			let seen = stalled.clone();
			// Accepts every request and never answers.
			let origin = host.listen(move |request, _response| {
				seen.borrow_mut().push(request.path().to_string());
				Box::pin(async { Ok(()) })
			});

			// Discovery.
			assert_eq!(
				run_until_stalled(&origin, &stalled, TestOAuthProvider::new("http://127.0.0.1/callback"))
					.await
					.as_deref(),
				Some("/.well-known/oauth-protected-resource/mcp")
			);

			// A failed refresh otherwise falls back to a new authorization.
			let refreshing = TestOAuthProvider::with_client("http://127.0.0.1/callback");
			*refreshing.token_set.borrow_mut() = Some(tokens("a1", Some("r1"), None));
			*refreshing.discovery.borrow_mut() = Some(OAuthDiscoveryState {
				authorization_server_url: origin.clone(),
				authorization_server_metadata: Some(metadata(&origin)),
				..Default::default()
			});
			assert_eq!(
				run_until_stalled(&origin, &stalled, refreshing).await.as_deref(),
				Some("/token")
			);
		});
	}
}
