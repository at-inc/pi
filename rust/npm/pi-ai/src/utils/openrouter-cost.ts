// utils/openrouter-cost: OpenRouter accounting in the Rust core; the reported cost is written to the caller's usage.

import type * as ts from "@at-inc/pi-ai/utils/openrouter-cost";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/openrouter-cost";

export const isOpenRouterModel: typeof ts.isOpenRouterModel = (model) => wasm.aiIsOpenRouterModel(model);

export const applyOpenRouterReportedCost: typeof ts.applyOpenRouterReportedCost = (usage, reported) =>
	wasm.aiApplyOpenRouterReportedCost(usage, reported);
