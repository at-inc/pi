// providers/cloudflare-auth: the Cloudflare api-key auth of the Workers AI and AI Gateway providers, over the Rust core.

import type * as ts from "@at-inc/pi-ai/providers/cloudflare-auth";
import { wasm } from "@at-inc/pi-core";
import { apiKeyAuthOf } from "../internal/auth.ts";

export type * from "@at-inc/pi-ai/providers/cloudflare-auth";

export const cloudflareWorkersAIAuth: typeof ts.cloudflareWorkersAIAuth = () =>
	apiKeyAuthOf(wasm.aiCloudflareWorkersAIAuth());

export const cloudflareAIGatewayAuth: typeof ts.cloudflareAIGatewayAuth = () =>
	apiKeyAuthOf(wasm.aiCloudflareAIGatewayAuth());
