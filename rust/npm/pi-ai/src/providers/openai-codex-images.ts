// providers/openai-codex-images: `openaiCodexImagesProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/openai-codex-images";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/openai-codex-images";

export const openaiCodexImagesProvider: typeof ts.openaiCodexImagesProvider = () =>
	providerFromHandle(wasm.aiProvider("openai-codex-images")) as ReturnType<typeof ts.openaiCodexImagesProvider>;
