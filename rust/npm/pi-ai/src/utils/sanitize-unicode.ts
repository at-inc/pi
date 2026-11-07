// utils/sanitize-unicode: unpaired surrogates removed in the Rust core, on the string's UTF-16 code units.

import type * as ts from "@at-inc/pi-ai/utils/sanitize-unicode";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/sanitize-unicode";

export const sanitizeSurrogates: typeof ts.sanitizeSurrogates = (text) => wasm.aiSanitizeSurrogates(text);
