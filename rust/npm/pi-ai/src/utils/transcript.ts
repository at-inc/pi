// utils/transcript: the slice provides normalizeContext.

import type * as ts from "@at-inc/pi-ai/utils/transcript";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/transcript";

export const normalizeContext: typeof ts.normalizeContext = (context) => wasm.aiNormalizeContext(context);
