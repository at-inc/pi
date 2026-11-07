// providers/groq: `groqProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/groq";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/groq";

export const groqProvider: typeof ts.groqProvider = () =>
	providerFromHandle(wasm.aiProvider("groq")) as ReturnType<typeof ts.groqProvider>;
