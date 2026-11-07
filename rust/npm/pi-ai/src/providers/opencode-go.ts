// providers/opencode-go: `opencodeGoProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/opencode-go";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/opencode-go";

export const opencodeGoProvider: typeof ts.opencodeGoProvider = () =>
	providerFromHandle(wasm.aiProvider("opencode-go")) as ReturnType<typeof ts.opencodeGoProvider>;
