// providers/qwen-token-plan-cn: `qwenTokenPlanCnProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/qwen-token-plan-cn";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/qwen-token-plan-cn";

export const qwenTokenPlanCnProvider: typeof ts.qwenTokenPlanCnProvider = () =>
	providerFromHandle(wasm.aiProvider("qwen-token-plan-cn")) as ReturnType<typeof ts.qwenTokenPlanCnProvider>;
