// providers/radius: `radiusProvider` over the Rust provider.

import type * as ts from "@at-inc/pi-ai/providers/radius";
import { wasm } from "@at-inc/pi-core";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/radius";

export const radiusProvider: typeof ts.radiusProvider = (options = {}) =>
	providerFromHandle(wasm.aiRadiusProvider(options.id, options.name, options.gateway)) as ReturnType<
		typeof ts.radiusProvider
	>;
