// utils/version: the version of the Rust core's pi-ai.

import type * as ts from "@at-inc/pi-ai/utils/version";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/version";

export const PI_AI_VERSION: typeof ts.PI_AI_VERSION = wasm.aiPiAiVersion() as typeof ts.PI_AI_VERSION;
