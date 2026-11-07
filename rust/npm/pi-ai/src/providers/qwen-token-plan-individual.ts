// providers/qwen-token-plan-individual: `qwenTokenPlanIndividualProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/qwen-token-plan-individual";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/qwen-token-plan-individual";

export const qwenTokenPlanIndividualProvider: typeof ts.qwenTokenPlanIndividualProvider = () =>
	providerFromHandle(wasm.aiProvider("qwen-token-plan-individual")) as ReturnType<
		typeof ts.qwenTokenPlanIndividualProvider
	>;
