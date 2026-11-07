// providers/opencode: `opencodeProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/opencode";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/opencode";

export const opencodeProvider: typeof ts.opencodeProvider = () =>
	providerFromHandle(wasm.aiProvider("opencode")) as ReturnType<typeof ts.opencodeProvider>;
