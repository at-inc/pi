// providers/zai: `zaiProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/zai";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/zai";

export const zaiProvider: typeof ts.zaiProvider = () =>
	providerFromHandle(wasm.aiProvider("zai")) as ReturnType<typeof ts.zaiProvider>;
