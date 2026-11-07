// utils/uuid: UUIDv7 in the Rust core, with random bytes from `globalThis.crypto.getRandomValues` per call.

import type * as ts from "@at-inc/pi-ai/utils/uuid";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/uuid";

export const uuidv7: typeof ts.uuidv7 = (timestampMs) => wasm.aiUuidv7(timestampMs);
