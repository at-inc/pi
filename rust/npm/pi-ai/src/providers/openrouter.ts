// providers/openrouter: `openrouterProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/openrouter";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/openrouter";

export const openrouterProvider: typeof ts.openrouterProvider = () =>
	providerFromHandle(wasm.aiProvider("openrouter")) as ReturnType<typeof ts.openrouterProvider>;
