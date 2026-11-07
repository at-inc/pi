// providers/cerebras: `cerebrasProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/cerebras";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/cerebras";

export const cerebrasProvider: typeof ts.cerebrasProvider = () =>
	providerFromHandle(wasm.aiProvider("cerebras")) as ReturnType<typeof ts.cerebrasProvider>;
