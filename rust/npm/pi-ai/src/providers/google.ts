// providers/google: `googleProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/google";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/google";

export const googleProvider: typeof ts.googleProvider = () =>
	providerFromHandle(wasm.aiProvider("google")) as ReturnType<typeof ts.googleProvider>;
