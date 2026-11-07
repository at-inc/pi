// providers/mistral: `mistralProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/mistral";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/mistral";

export const mistralProvider: typeof ts.mistralProvider = () =>
	providerFromHandle(wasm.aiProvider("mistral")) as ReturnType<typeof ts.mistralProvider>;
