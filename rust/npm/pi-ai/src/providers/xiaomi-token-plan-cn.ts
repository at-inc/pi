// providers/xiaomi-token-plan-cn: `xiaomiTokenPlanCnProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/xiaomi-token-plan-cn";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/xiaomi-token-plan-cn";

export const xiaomiTokenPlanCnProvider: typeof ts.xiaomiTokenPlanCnProvider = () =>
	providerFromHandle(wasm.aiProvider("xiaomi-token-plan-cn")) as ReturnType<typeof ts.xiaomiTokenPlanCnProvider>;
