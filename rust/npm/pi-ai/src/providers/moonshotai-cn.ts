// providers/moonshotai-cn: `moonshotaiCnProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/moonshotai-cn";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/moonshotai-cn";

export const moonshotaiCnProvider: typeof ts.moonshotaiCnProvider = () =>
	providerFromHandle(wasm.aiProvider("moonshotai-cn")) as ReturnType<typeof ts.moonshotaiCnProvider>;
