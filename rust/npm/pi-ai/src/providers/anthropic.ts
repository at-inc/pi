// providers/anthropic: `anthropicProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/anthropic";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/anthropic";

export const anthropicProvider: typeof ts.anthropicProvider = () =>
	providerFromHandle(wasm.aiProvider("anthropic")) as ReturnType<typeof ts.anthropicProvider>;
