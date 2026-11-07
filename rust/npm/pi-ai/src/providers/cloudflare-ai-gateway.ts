// providers/cloudflare-ai-gateway: `cloudflareAIGatewayProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/cloudflare-ai-gateway";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/cloudflare-ai-gateway";

export const cloudflareAIGatewayProvider: typeof ts.cloudflareAIGatewayProvider = () =>
	providerFromHandle(wasm.aiProvider("cloudflare-ai-gateway")) as ReturnType<typeof ts.cloudflareAIGatewayProvider>;
