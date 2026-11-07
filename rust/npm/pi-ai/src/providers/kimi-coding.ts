// providers/kimi-coding: `kimiCodingProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/kimi-coding";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/kimi-coding";

export const kimiCodingProvider: typeof ts.kimiCodingProvider = () =>
	providerFromHandle(wasm.aiProvider("kimi-coding")) as ReturnType<typeof ts.kimiCodingProvider>;
