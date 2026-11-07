// providers/xiaomi: `xiaomiProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/xiaomi";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/xiaomi";

export const xiaomiProvider: typeof ts.xiaomiProvider = () =>
	providerFromHandle(wasm.aiProvider("xiaomi")) as ReturnType<typeof ts.xiaomiProvider>;
