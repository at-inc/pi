// providers/vercel-ai-gateway: `vercelAIGatewayProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/vercel-ai-gateway";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/vercel-ai-gateway";

export const vercelAIGatewayProvider: typeof ts.vercelAIGatewayProvider = () =>
	providerFromHandle(wasm.aiProvider("vercel-ai-gateway")) as ReturnType<typeof ts.vercelAIGatewayProvider>;
