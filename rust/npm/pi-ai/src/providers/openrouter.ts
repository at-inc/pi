// providers/openrouter: `openrouterProvider` and `personalizeOpenRouterCatalog` over the Rust provider module.

import type { AnyModel } from "@at-inc/pi-ai";
import type * as ts from "@at-inc/pi-ai/providers/openrouter";
import { wasm } from "@at-inc/pi-core";
import { rethrow } from "../internal/errors.ts";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/openrouter";

export const openrouterProvider: typeof ts.openrouterProvider = () =>
	providerFromHandle(wasm.aiProvider("openrouter")) as ReturnType<typeof ts.openrouterProvider>;

export const personalizeOpenRouterCatalog: typeof ts.personalizeOpenRouterCatalog = (catalog, context) =>
	wasm.aiPersonalizeOpenRouterCatalog(catalog as AnyModel[], context).catch(rethrow);
