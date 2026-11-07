// providers/openai: `openaiProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/openai";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/openai";

export const openaiProvider: typeof ts.openaiProvider = () =>
	providerFromHandle(wasm.aiProvider("openai")) as ReturnType<typeof ts.openaiProvider>;
