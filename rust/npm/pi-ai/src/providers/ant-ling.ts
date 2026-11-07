// providers/ant-ling: `antLingProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/ant-ling";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/ant-ling";

export const antLingProvider: typeof ts.antLingProvider = () =>
	providerFromHandle(wasm.aiProvider("ant-ling")) as ReturnType<typeof ts.antLingProvider>;
