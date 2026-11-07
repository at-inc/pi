// utils/headers: header records in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/headers";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/headers";

export const headersToRecord: typeof ts.headersToRecord = (headers) => wasm.aiHeadersToRecord(headers);

export const providerHeadersToRecord: typeof ts.providerHeadersToRecord = (...headerSources) =>
	wasm.aiProviderHeadersToRecord(headerSources);
