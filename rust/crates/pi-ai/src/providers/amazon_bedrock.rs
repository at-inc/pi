//! Port of `packages/ai/src/providers/amazon-bedrock.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `Provider<TApi>`'s type parameter only narrows the API type, so the factory returns a plain `Provider`.
//! - `createProvider()` throws only without an implementation, which this factory always passes, so the factory
//!   returns the provider instead of a `PiResult`.

use std::rc::Rc;

use pi_host::{AbortSignal, PiError, PiResult};

use crate::api::bedrock_converse_stream_lazy::bedrock_converse_stream_api;
use crate::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthEvent, AuthInfoLink, AuthPrompt, AuthPromptKind,
	AuthResult, AuthSelectOption, ModelAuth, ProviderAuth,
};
use crate::models::{CreateProviderOptions, Provider, ProviderApi, create_provider};
use crate::providers::amazon_bedrock_models::AMAZON_BEDROCK_MODELS;
use crate::types::{AnyModel, ProviderEnv};

fn select_option(id: &str, label: &str) -> AuthSelectOption {
	AuthSelectOption {
		id: id.into(),
		label: label.into(),
		description: None,
	}
}

/// `env(name)` of `resolve`: the ambient value, checked against the abort signal before and after the read.
async fn checked_env(ctx: &Rc<dyn AuthContext>, signal: &AbortSignal, name: &str) -> PiResult<Option<String>> {
	signal.throw_if_aborted()?;
	let value = ctx.env(name).await;
	signal.throw_if_aborted()?;
	Ok(value)
}

fn ambient(source: &str, env: Option<ProviderEnv>) -> Option<AuthResult> {
	Some(AuthResult {
		auth: ModelAuth::default(),
		env,
		source: Some(source.into()),
	})
}

fn truthy(value: &Option<String>) -> bool {
	value.as_ref().is_some_and(|value| !value.is_empty())
}

/// Bedrock accepts a bearer token or the AWS SDK's default credential chain.
/// The login flow can store a token/profile choice; resolve also detects ambient
/// AWS credentials without copying them into pi's credential store.
fn bedrock_auth() -> ApiKeyAuth {
	ApiKeyAuth {
		name: "AWS credentials or bearer token".into(),
		login: Some(Rc::new(|interaction| {
			Box::pin(async move {
				interaction.signal.throw_if_aborted()?;
				let method = (interaction.prompt)(AuthPrompt::new(AuthPromptKind::Select {
					message: "Select Amazon Bedrock authentication method:".into(),
					options: vec![
						select_option("bearer-token", "Bearer token"),
						select_option("aws-profile", "AWS profile"),
						select_option("credential-chain", "Existing AWS credential chain"),
					],
				}))
				.await?;
				interaction.signal.throw_if_aborted()?;
				if method == "bearer-token" {
					return Ok(ApiKeyCredential {
						key: Some(
							(interaction.prompt)(AuthPrompt::new(AuthPromptKind::Secret {
								message: "Enter Amazon Bedrock bearer token".into(),
								placeholder: None,
							}))
							.await?,
						),
						env: None,
						extra: Default::default(),
					});
				}
				(interaction.notify)(AuthEvent::Info {
					message: "Amazon Bedrock supports AWS profiles, IAM credentials, and role-based credentials."
						.into(),
					links: Some(vec![AuthInfoLink {
						label: Some("AWS credential provider chain".into()),
						url: "https://docs.aws.amazon.com/sdkref/latest/guide/standardized-credentials.html".into(),
					}]),
				});
				if method == "aws-profile" {
					let profile = (interaction.prompt)(AuthPrompt::new(AuthPromptKind::Text {
						message: "Enter AWS profile name".into(),
						placeholder: None,
					}))
					.await?;
					return Ok(ApiKeyCredential {
						key: None,
						env: Some(ProviderEnv::from([("AWS_PROFILE".to_string(), profile)])),
						extra: Default::default(),
					});
				}
				if method != "credential-chain" {
					return Err(PiError::new(format!("Unknown Amazon Bedrock auth method: {method}")));
				}
				(interaction.prompt)(AuthPrompt::new(AuthPromptKind::Text {
					message: "Configure AWS credentials, then press Enter to continue".into(),
					placeholder: None,
				}))
				.await?;
				Ok(ApiKeyCredential::default())
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
					if let Some(credential) = &credential
						&& let Some(key) = credential.key.as_ref().filter(|key| !key.is_empty())
					{
						return Ok(Some(AuthResult {
							auth: ModelAuth {
								api_key: Some(key.clone()),
								..ModelAuth::default()
							},
							env: credential.env.clone(),
							source: Some("stored credential".into()),
						}));
					}
					if truthy(&checked_env(&ctx, &signal, "AWS_BEARER_TOKEN_BEDROCK").await?) {
						return Ok(ambient("AWS_BEARER_TOKEN_BEDROCK", None));
					}
					let credential_env = credential.as_ref().and_then(|credential| credential.env.clone());
					let stored_profile = credential_env.as_ref().and_then(|env| env.get("AWS_PROFILE").cloned());
					let profile = match stored_profile.clone() {
						Some(profile) => Some(profile),
						None => checked_env(&ctx, &signal, "AWS_PROFILE").await?,
					};
					if truthy(&profile) {
						let source = if truthy(&stored_profile) {
							"stored credential"
						} else {
							"AWS_PROFILE"
						};
						return Ok(ambient(source, credential_env));
					}
					if truthy(&checked_env(&ctx, &signal, "AWS_ACCESS_KEY_ID").await?)
						&& truthy(&checked_env(&ctx, &signal, "AWS_SECRET_ACCESS_KEY").await?)
					{
						return Ok(ambient("AWS access keys", None));
					}
					if truthy(&checked_env(&ctx, &signal, "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").await?) {
						return Ok(ambient("ECS task role", None));
					}
					if truthy(&checked_env(&ctx, &signal, "AWS_CONTAINER_CREDENTIALS_FULL_URI").await?) {
						return Ok(ambient("ECS task role", None));
					}
					if truthy(&checked_env(&ctx, &signal, "AWS_WEB_IDENTITY_TOKEN_FILE").await?) {
						return Ok(ambient("web identity token", None));
					}
					Ok(None)
				})
			},
		),
	}
}

pub fn amazon_bedrock_provider() -> Provider {
	create_provider(CreateProviderOptions {
		id: "amazon-bedrock".into(),
		name: Some("Amazon Bedrock".into()),
		auth: ProviderAuth {
			api_key: Some(bedrock_auth()),
			oauth: None,
		},
		models: AMAZON_BEDROCK_MODELS.values().cloned().map(AnyModel::Chat).collect(),
		api: Some(ProviderApi::Single(bedrock_converse_stream_api())),
		..CreateProviderOptions::default()
	})
	.expect("a provider with an api implementation always builds")
}
