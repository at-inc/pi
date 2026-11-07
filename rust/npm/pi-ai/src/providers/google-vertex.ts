// providers/google-vertex: `googleVertexProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/google-vertex";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/google-vertex";

export const googleVertexProvider: typeof ts.googleVertexProvider = () =>
	providerFromHandle(wasm.aiProvider("google-vertex")) as ReturnType<typeof ts.googleVertexProvider>;
