// providers/typesafe: `typesafeProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/typesafe";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/typesafe";

export const typesafeProvider: typeof ts.typesafeProvider = () =>
	providerFromHandle(wasm.aiProvider("typesafe")) as ReturnType<typeof ts.typesafeProvider>;
