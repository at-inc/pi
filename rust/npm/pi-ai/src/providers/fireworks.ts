// providers/fireworks: `fireworksProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/fireworks";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/fireworks";

export const fireworksProvider: typeof ts.fireworksProvider = () =>
	providerFromHandle(wasm.aiProvider("fireworks")) as ReturnType<typeof ts.fireworksProvider>;
