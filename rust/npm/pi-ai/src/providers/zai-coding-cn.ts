// providers/zai-coding-cn: `zaiCodingCnProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/zai-coding-cn";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/zai-coding-cn";

export const zaiCodingCnProvider: typeof ts.zaiCodingCnProvider = () =>
	providerFromHandle(wasm.aiProvider("zai-coding-cn")) as ReturnType<typeof ts.zaiCodingCnProvider>;
