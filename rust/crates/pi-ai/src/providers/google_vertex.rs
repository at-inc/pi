//! Port of `packages/ai/src/providers/google-vertex.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.
//! - `vertexAuth` is a module constant in the TS; Rust builds it in `vertex_auth()` because it holds `Rc` callbacks.

use std::rc::Rc;

use pi_host::{AbortSignal, PiError, PiResult};

use crate::api::google_vertex_lazy::google_vertex_api;
use crate::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthEvent, AuthInfoLink, AuthPrompt, AuthResult,
	AuthSelectOption, ModelAuth, ProviderAuth,
};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::google_vertex_models::GOOGLE_VERTEX_MODELS;
use crate::types::{AnyModel, ProviderEnv};

const VERTEX_ADC_PATH: &str = "~/.config/gcloud/application_default_credentials.json";

fn select_option(id: &str, label: &str) -> AuthSelectOption {
	AuthSelectOption {
		id: id.into(),
		label: label.into(),
		description: None,
	}
}

fn text_prompt(message: &str) -> AuthPrompt {
	AuthPrompt::Text {
		message: message.into(),
		placeholder: None,
		signal: None,
	}
}

/// `env(name)` of `resolve`: the ambient value, checked against the abort signal before and after the read.
async fn checked_env(ctx: &Rc<dyn AuthContext>, signal: &AbortSignal, name: &str) -> PiResult<Option<String>> {
	signal.throw_if_aborted()?;
	let value = ctx.env(name).await;
	signal.throw_if_aborted()?;
	Ok(value)
}

fn truthy(value: &Option<String>) -> bool {
	value.as_ref().is_some_and(|value| !value.is_empty())
}

/// Vertex accepts an explicit API key or Application Default Credentials
/// (`gcloud auth application-default login`). ADC additionally requires
/// project and location env vars, which the implementation reads itself.
fn vertex_auth() -> ApiKeyAuth {
	ApiKeyAuth {
		name: "Google Cloud credentials".into(),
		login: Some(Rc::new(|interaction| {
			Box::pin(async move {
				interaction.signal.throw_if_aborted()?;
				let method = (interaction.prompt)(AuthPrompt::Select {
					message: "Select Google Vertex AI authentication method:".into(),
					options: vec![
						select_option("api-key", "Google Cloud API key"),
						select_option("adc", "Application Default Credentials"),
						select_option("service-account", "Service account credentials file"),
					],
					signal: None,
				})
				.await?;
				interaction.signal.throw_if_aborted()?;
				if method == "api-key" {
					return Ok(ApiKeyCredential {
						key: Some(
							(interaction.prompt)(AuthPrompt::Secret {
								message: "Enter Google Cloud API key".into(),
								placeholder: None,
								signal: None,
							})
							.await?,
						),
						env: None,
					});
				}
				if method != "adc" && method != "service-account" {
					return Err(PiError::new(format!("Unknown Google Vertex AI auth method: {method}")));
				}
				(interaction.notify)(AuthEvent::Info {
					message: if method == "adc" {
						"Run `gcloud auth application-default login`, then provide the project and location.".into()
					} else {
						"Provide a service account credentials file, project, and location.".into()
					},
					links: Some(vec![AuthInfoLink {
						label: Some("Application Default Credentials".into()),
						url: "https://cloud.google.com/docs/authentication/provide-credentials-adc".into(),
					}]),
				});
				let project = (interaction.prompt)(text_prompt("Enter Google Cloud project ID")).await?;
				let location = (interaction.prompt)(text_prompt("Enter Google Cloud location")).await?;
				let credentials_path = if method == "service-account" {
					Some((interaction.prompt)(text_prompt("Enter service account credentials file path")).await?)
				} else {
					None
				};
				let mut env = ProviderEnv::from([
					("GOOGLE_CLOUD_PROJECT".to_string(), project),
					("GOOGLE_CLOUD_LOCATION".to_string(), location),
				]);
				if let Some(credentials_path) = credentials_path.filter(|path| !path.is_empty()) {
					env.insert("GOOGLE_APPLICATION_CREDENTIALS".into(), credentials_path);
				}
				Ok(ApiKeyCredential {
					key: None,
					env: Some(env),
				})
			})
		})),
		check: None,
		resolve: Rc::new(
			|ApiKeyAuthInput {
			     ctx,
			     credential,
			     signal,
			 }| {
				Box::pin(async move {
					let stored_key = credential.as_ref().and_then(|credential| credential.key.clone());
					let key = match stored_key.clone() {
						Some(key) => Some(key),
						None => checked_env(&ctx, &signal, "GOOGLE_CLOUD_API_KEY").await?,
					};
					if let Some(key) = key.filter(|key| !key.is_empty()) {
						return Ok(Some(AuthResult {
							auth: ModelAuth {
								api_key: Some(key),
								..ModelAuth::default()
							},
							env: None,
							source: Some(if truthy(&stored_key) {
								"stored credential".into()
							} else {
								"GOOGLE_CLOUD_API_KEY".into()
							}),
						}));
					}

					let credential_env = credential.as_ref().and_then(|credential| credential.env.clone());
					let stored = |name: &str| credential_env.as_ref().and_then(|env| env.get(name).cloned());
					let adc_path = match stored("GOOGLE_APPLICATION_CREDENTIALS") {
						Some(path) => Some(path),
						None => checked_env(&ctx, &signal, "GOOGLE_APPLICATION_CREDENTIALS").await?,
					};
					signal.throw_if_aborted()?;
					let has_credentials = ctx.file_exists(adc_path.as_deref().unwrap_or(VERTEX_ADC_PATH)).await;
					signal.throw_if_aborted()?;
					let project = match stored("GOOGLE_CLOUD_PROJECT") {
						Some(project) => Some(project),
						None => match checked_env(&ctx, &signal, "GOOGLE_CLOUD_PROJECT").await? {
							Some(project) => Some(project),
							None => checked_env(&ctx, &signal, "GCLOUD_PROJECT").await?,
						},
					};
					let location = match stored("GOOGLE_CLOUD_LOCATION") {
						Some(location) => Some(location),
						None => checked_env(&ctx, &signal, "GOOGLE_CLOUD_LOCATION").await?,
					};
					if has_credentials && truthy(&project) && truthy(&location) {
						return Ok(Some(AuthResult {
							auth: ModelAuth::default(),
							env: credential_env,
							source: Some(if credential.is_some() {
								"stored credential".into()
							} else {
								"gcloud application default credentials".into()
							}),
						}));
					}
					Ok(None)
				})
			},
		),
	}
}

pub fn google_vertex_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "google-vertex".into(),
		name: Some("Google Vertex AI".into()),
		auth: ProviderAuth {
			api_key: Some(vertex_auth()),
			oauth: None,
		},
		models: GOOGLE_VERTEX_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(google_vertex_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
