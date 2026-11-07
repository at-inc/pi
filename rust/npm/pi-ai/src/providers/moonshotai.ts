// providers/moonshotai: `moonshotaiProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/moonshotai";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/moonshotai";

export const moonshotaiProvider: typeof ts.moonshotaiProvider = () =>
	providerFromHandle(wasm.aiProvider("moonshotai")) as ReturnType<typeof ts.moonshotaiProvider>;
