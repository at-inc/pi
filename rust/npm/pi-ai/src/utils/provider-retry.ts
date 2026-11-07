// utils/provider-retry: the SDK retry policy in the Rust core; `request` is a JS function it awaits, and the value it
// threw last is rethrown as is.

import type * as ts from "@at-inc/pi-ai/utils/provider-retry";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/provider-retry";

export const retryProviderRequest = ((request: () => Promise<unknown>, options?: unknown) =>
	wasm.aiRetryProviderRequest(request, options)) as typeof ts.retryProviderRequest;
