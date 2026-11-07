// providers/together: `togetherProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/together";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/together";

export const togetherProvider: typeof ts.togetherProvider = () =>
	providerFromHandle(wasm.aiProvider("together")) as ReturnType<typeof ts.togetherProvider>;
