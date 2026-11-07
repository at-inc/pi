// providers/huggingface: `huggingfaceProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/huggingface";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/huggingface";

export const huggingfaceProvider: typeof ts.huggingfaceProvider = () =>
	providerFromHandle(wasm.aiProvider("huggingface")) as ReturnType<typeof ts.huggingfaceProvider>;
