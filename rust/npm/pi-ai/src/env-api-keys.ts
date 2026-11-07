// env-api-keys: the environment variables that hold each provider's API key, read by the Rust core.

import type * as ts from "@at-inc/pi-ai/compat";
import { wasm } from "@at-inc/pi-core";

export const ANTHROPIC_AUTH_TOKEN_ENV: typeof ts.ANTHROPIC_AUTH_TOKEN_ENV = "ANTHROPIC_AUTH_TOKEN";
export const ANTHROPIC_OAUTH_TOKEN_ENV: typeof ts.ANTHROPIC_OAUTH_TOKEN_ENV = "ANTHROPIC_OAUTH_TOKEN";
export const ANTHROPIC_API_KEY_ENV: typeof ts.ANTHROPIC_API_KEY_ENV = "ANTHROPIC_API_KEY";
export const ANTHROPIC_FEDERATION_RULE_ID_ENV: typeof ts.ANTHROPIC_FEDERATION_RULE_ID_ENV =
	"ANTHROPIC_FEDERATION_RULE_ID";
export const ANTHROPIC_ORGANIZATION_ID_ENV: typeof ts.ANTHROPIC_ORGANIZATION_ID_ENV = "ANTHROPIC_ORGANIZATION_ID";
export const ANTHROPIC_SERVICE_ACCOUNT_ID_ENV: typeof ts.ANTHROPIC_SERVICE_ACCOUNT_ID_ENV =
	"ANTHROPIC_SERVICE_ACCOUNT_ID";
export const ANTHROPIC_IDENTITY_TOKEN_FILE_ENV: typeof ts.ANTHROPIC_IDENTITY_TOKEN_FILE_ENV =
	"ANTHROPIC_IDENTITY_TOKEN_FILE";
export const ANTHROPIC_WORKSPACE_ID_ENV: typeof ts.ANTHROPIC_WORKSPACE_ID_ENV = "ANTHROPIC_WORKSPACE_ID";

export const findEnvKeys: typeof ts.findEnvKeys = (provider: string, env?: Parameters<typeof ts.getEnvApiKey>[1]) =>
	wasm.aiFindEnvKeys(provider, env);

export const getEnvApiKey: typeof ts.getEnvApiKey = (provider: string, env?: Parameters<typeof ts.getEnvApiKey>[1]) =>
	wasm.aiGetEnvApiKey(provider, env);
