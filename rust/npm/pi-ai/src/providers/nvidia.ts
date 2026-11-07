// providers/nvidia: `nvidiaProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/nvidia";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/nvidia";

export const nvidiaProvider: typeof ts.nvidiaProvider = () =>
	providerFromHandle(wasm.aiProvider("nvidia")) as ReturnType<typeof ts.nvidiaProvider>;
