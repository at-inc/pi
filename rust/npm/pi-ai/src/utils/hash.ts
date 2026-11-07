// utils/hash: the hash in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/hash";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/hash";

export const shortHash: typeof ts.shortHash = (str) => wasm.aiShortHash(str);
