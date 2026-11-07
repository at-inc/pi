// providers/amazon-bedrock: `amazonBedrockProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/amazon-bedrock";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/amazon-bedrock";

export const amazonBedrockProvider: typeof ts.amazonBedrockProvider = () =>
	providerFromHandle(wasm.aiProvider("amazon-bedrock")) as ReturnType<typeof ts.amazonBedrockProvider>;
