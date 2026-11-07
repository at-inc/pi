// providers/qwen-token-plan: `qwenTokenPlanProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/qwen-token-plan";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/qwen-token-plan";

export const qwenTokenPlanProvider: typeof ts.qwenTokenPlanProvider = () =>
	providerFromHandle(wasm.aiProvider("qwen-token-plan")) as ReturnType<typeof ts.qwenTokenPlanProvider>;
