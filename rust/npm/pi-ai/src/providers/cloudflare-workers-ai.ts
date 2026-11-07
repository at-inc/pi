// providers/cloudflare-workers-ai: `cloudflareWorkersAIProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/cloudflare-workers-ai";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/cloudflare-workers-ai";

export const cloudflareWorkersAIProvider: typeof ts.cloudflareWorkersAIProvider = () =>
	providerFromHandle(wasm.aiProvider("cloudflare-workers-ai")) as ReturnType<typeof ts.cloudflareWorkersAIProvider>;
