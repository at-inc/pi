// providers/deepseek: `deepseekProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/deepseek";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/deepseek";

export const deepseekProvider: typeof ts.deepseekProvider = () =>
	providerFromHandle(wasm.aiProvider("deepseek")) as ReturnType<typeof ts.deepseekProvider>;
