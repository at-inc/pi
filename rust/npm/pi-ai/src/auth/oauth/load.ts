// auth/oauth/load: the OAuth flow loaders. The bundled loaders are registered in the Rust core, which the Rust
// providers' lazy OAuth reads too.

import type * as ts from "@at-inc/pi-ai/auth/oauth/load";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const registerBundledOAuthFlowLoaders: typeof ts.registerBundledOAuthFlowLoaders = (loaders) =>
	wasm.aiRegisterBundledOAuthFlowLoaders(loaders);

export const loadAnthropicOAuth: typeof ts.loadAnthropicOAuth = async () =>
	oauthAuthOf(await wasm.aiLoadAnthropicOAuth());

export const loadOpenAICodexOAuth: typeof ts.loadOpenAICodexOAuth = async () =>
	oauthAuthOf(await wasm.aiLoadOpenAICodexOAuth());

export const loadOpenAIChatGPTOAuth: typeof ts.loadOpenAIChatGPTOAuth = async () =>
	oauthAuthOf(await wasm.aiLoadOpenAIChatGPTOAuth());

export const loadGitHubCopilotOAuth: typeof ts.loadGitHubCopilotOAuth = async () =>
	oauthAuthOf(await wasm.aiLoadGitHubCopilotOAuth());

export const loadOpenRouterOAuth: typeof ts.loadOpenRouterOAuth = async () =>
	oauthAuthOf(await wasm.aiLoadOpenRouterOAuth());

export const loadKimiCodingOAuth: typeof ts.loadKimiCodingOAuth = async () =>
	oauthAuthOf(await wasm.aiLoadKimiCodingOAuth());

export const loadMetaOAuth: typeof ts.loadMetaOAuth = async () => oauthAuthOf(await wasm.aiLoadMetaOAuth());

export const loadXaiOAuth: typeof ts.loadXaiOAuth = async () => oauthAuthOf(await wasm.aiLoadXaiOAuth());

export const loadRadiusOAuth: typeof ts.loadRadiusOAuth = async (options) =>
	oauthAuthOf(await wasm.aiLoadRadiusOAuth(options));
