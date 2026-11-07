// providers/baseten: `basetenProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/baseten";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/baseten";

export const basetenProvider: typeof ts.basetenProvider = () =>
	providerFromHandle(wasm.aiProvider("baseten")) as ReturnType<typeof ts.basetenProvider>;
