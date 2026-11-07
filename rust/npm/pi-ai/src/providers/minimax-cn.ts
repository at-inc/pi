// providers/minimax-cn: `minimaxCnProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/minimax-cn";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/minimax-cn";

export const minimaxCnProvider: typeof ts.minimaxCnProvider = () =>
	providerFromHandle(wasm.aiProvider("minimax-cn")) as ReturnType<typeof ts.minimaxCnProvider>;
