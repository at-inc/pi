// providers/opencode-headers: `withOpenCodeSessionHeader` over the Rust provider module.

import type * as ts from "@at-inc/pi-ai/providers/opencode-headers";
import { wasm } from "@at-inc/pi-core";
import { nativeApi, nativeOf } from "../internal/native-api.ts";

export const withOpenCodeSessionHeader: typeof ts.withOpenCodeSessionHeader = (streams) =>
	nativeApi(wasm.aiWithOpenCodeSessionHeader(nativeOf(streams)?.share(), streams));
