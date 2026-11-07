//! Port of `packages/ai/src/providers/cloudflare-auth.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `resolveCloudflareEnv`'s anonymous result type is the private `ResolvedCloudflareEnv` struct.

use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::{AbortSignal, PiResult};

use crate::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthPrompt, AuthResult, ModelAuth,
};
use crate::types::ProviderEnv;

const CLOUDFLARE_API_KEY: &str = "CLOUDFLARE_API_KEY";
const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
const CLOUDFLARE_GATEWAY_ID: &str = "CLOUDFLARE_GATEWAY_ID";

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloudflareAuthKind {
	WorkersAi,
	AiGateway,
}

async fn resolve_value(
	name: &str,
	ctx: &Rc<dyn AuthContext>,
	credential: Option<&ApiKeyCredential>,
	signal: &AbortSignal,
) -> PiResult<Option<String>> {
	// Per-field merge: prefer the credential value, fall back to ambient env.
	// A credential carrying only the API key must still pick up the account /
	// gateway id from the environment.
	let from_credential = credential.and_then(|credential| {
		if name == CLOUDFLARE_API_KEY {
			credential.key.clone()
		} else {
			credential.env.as_ref().and_then(|env| env.get(name).cloned())
		}
	});
	if from_credential.is_some() {
		return Ok(from_credential);
	}
	signal.throw_if_aborted()?;
	let value = ctx.env(name).await;
	signal.throw_if_aborted()?;
	Ok(value)
}

struct ResolvedCloudflareEnv {
	api_key: String,
	env: ProviderEnv,
	source: String,
}

async fn resolve_cloudflare_env(
	kind: CloudflareAuthKind,
	ctx: &Rc<dyn AuthContext>,
	credential: Option<&ApiKeyCredential>,
	signal: &AbortSignal,
) -> PiResult<Option<ResolvedCloudflareEnv>> {
	let api_key = resolve_value(CLOUDFLARE_API_KEY, ctx, credential, signal).await?;
	let account_id = resolve_value(CLOUDFLARE_ACCOUNT_ID, ctx, credential, signal).await?;
	let gateway_id = if kind == CloudflareAuthKind::AiGateway {
		resolve_value(CLOUDFLARE_GATEWAY_ID, ctx, credential, signal).await?
	} else {
		None
	};

	let (Some(api_key), Some(account_id)) = (
		api_key.filter(|value| !value.is_empty()),
		account_id.filter(|value| !value.is_empty()),
	) else {
		return Ok(None);
	};
	let gateway_id = gateway_id.filter(|value| !value.is_empty());
	if kind == CloudflareAuthKind::AiGateway && gateway_id.is_none() {
		return Ok(None);
	}

	let mut env = ProviderEnv::from([(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id)]);
	if let Some(gateway_id) = gateway_id {
		env.insert(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id);
	}
	Ok(Some(ResolvedCloudflareEnv {
		api_key,
		env,
		source: if credential.is_some() {
			"stored credential".into()
		} else {
			CLOUDFLARE_API_KEY.into()
		},
	}))
}

fn text_prompt(message: &str) -> AuthPrompt {
	AuthPrompt::Text {
		message: message.into(),
		placeholder: None,
		signal: None,
	}
}

fn secret_prompt(message: &str) -> AuthPrompt {
	AuthPrompt::Secret {
		message: message.into(),
		placeholder: None,
		signal: None,
	}
}

pub fn cloudflare_workers_ai_auth() -> ApiKeyAuth {
	ApiKeyAuth {
		name: "Cloudflare API key".into(),
		login: Some(Rc::new(|interaction| {
			Box::pin(async move {
				let key = (interaction.prompt)(secret_prompt("Enter Cloudflare API key")).await?;
				let account_id = (interaction.prompt)(text_prompt("Enter Cloudflare account ID")).await?;
				Ok(ApiKeyCredential {
					key: Some(key),
					env: Some(ProviderEnv::from([(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id)])),
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
					let Some(resolved) =
						resolve_cloudflare_env(CloudflareAuthKind::WorkersAi, &ctx, credential.as_ref(), &signal)
							.await?
					else {
						return Ok(None);
					};
					Ok(Some(AuthResult {
						auth: ModelAuth {
							api_key: Some(resolved.api_key),
							..ModelAuth::default()
						},
						env: Some(resolved.env),
						source: Some(resolved.source),
					}))
				})
			},
		),
	}
}

pub fn cloudflare_ai_gateway_auth() -> ApiKeyAuth {
	ApiKeyAuth {
		name: "Cloudflare API key".into(),
		login: Some(Rc::new(|interaction| {
			Box::pin(async move {
				let key = (interaction.prompt)(secret_prompt("Enter Cloudflare API key")).await?;
				let account_id = (interaction.prompt)(text_prompt("Enter Cloudflare account ID")).await?;
				let gateway_id = (interaction.prompt)(text_prompt("Enter Cloudflare AI Gateway ID")).await?;
				Ok(ApiKeyCredential {
					key: Some(key),
					env: Some(ProviderEnv::from([
						(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id),
						(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id),
					])),
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
					let Some(resolved) =
						resolve_cloudflare_env(CloudflareAuthKind::AiGateway, &ctx, credential.as_ref(), &signal)
							.await?
					else {
						return Ok(None);
					};
					Ok(Some(AuthResult {
						auth: ModelAuth {
							headers: Some(IndexMap::from([
								(
									"cf-aig-authorization".to_string(),
									Some(format!("Bearer {}", resolved.api_key)),
								),
								("Authorization".to_string(), None),
								("x-api-key".to_string(), None),
							])),
							..ModelAuth::default()
						},
						env: Some(resolved.env),
						source: Some(resolved.source),
					}))
				})
			},
		),
	}
}
