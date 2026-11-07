// utils/string-enum: the schema built in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/string-enum";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/string-enum";

export const StringEnum = ((values: readonly string[], options?: { description?: string; default?: string }) =>
	wasm.aiStringEnum(values, options)) as typeof ts.StringEnum;
