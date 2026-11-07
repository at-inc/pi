// providers/azure: `azureProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/azure";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/azure";

export const azureProvider: typeof ts.azureProvider = () =>
	providerFromHandle(wasm.aiProvider("azure")) as ReturnType<typeof ts.azureProvider>;
