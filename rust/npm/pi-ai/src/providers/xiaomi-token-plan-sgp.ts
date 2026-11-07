// providers/xiaomi-token-plan-sgp: `xiaomiTokenPlanSgpProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/xiaomi-token-plan-sgp";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/xiaomi-token-plan-sgp";

export const xiaomiTokenPlanSgpProvider: typeof ts.xiaomiTokenPlanSgpProvider = () =>
	providerFromHandle(wasm.aiProvider("xiaomi-token-plan-sgp")) as ReturnType<typeof ts.xiaomiTokenPlanSgpProvider>;
