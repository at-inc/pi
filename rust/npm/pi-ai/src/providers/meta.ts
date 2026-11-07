// providers/meta: `metaProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/meta";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/meta";

export const metaProvider: typeof ts.metaProvider = () =>
	providerFromHandle(wasm.aiProvider("meta")) as ReturnType<typeof ts.metaProvider>;
