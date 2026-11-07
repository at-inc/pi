// providers/minimax: `minimaxProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/minimax";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/minimax";

export const minimaxProvider: typeof ts.minimaxProvider = () =>
	providerFromHandle(wasm.aiProvider("minimax")) as ReturnType<typeof ts.minimaxProvider>;
