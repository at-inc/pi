// providers/xiaomi-token-plan-ams: `xiaomiTokenPlanAmsProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/xiaomi-token-plan-ams";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/xiaomi-token-plan-ams";

export const xiaomiTokenPlanAmsProvider: typeof ts.xiaomiTokenPlanAmsProvider = () =>
	providerFromHandle(wasm.aiProvider("xiaomi-token-plan-ams")) as ReturnType<typeof ts.xiaomiTokenPlanAmsProvider>;
