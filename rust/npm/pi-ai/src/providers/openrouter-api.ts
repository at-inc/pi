// providers/openrouter-api: `openRouterCost`, over the Rust core.

import type * as ts from "@at-inc/pi-ai/providers/openrouter-api";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/providers/openrouter-api";

export const openRouterCost: typeof ts.openRouterCost = (pricing) => wasm.aiOpenRouterCost(pricing);
