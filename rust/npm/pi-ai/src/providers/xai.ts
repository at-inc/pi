// providers/xai: `xaiProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/xai";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/xai";

export const xaiProvider: typeof ts.xaiProvider = () =>
	providerFromHandle(wasm.aiProvider("xai")) as ReturnType<typeof ts.xaiProvider>;
