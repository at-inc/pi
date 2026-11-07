// providers/cloudflare-workers-ai.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/cloudflare-workers-ai.models";
import { wasm } from "@at-inc/pi-core";

export const CLOUDFLARE_WORKERS_AI_MODELS: typeof ts.CLOUDFLARE_WORKERS_AI_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"cloudflare-workers-ai",
);

export const CLOUDFLARE_WORKERS_AI_IMAGE_MODELS: typeof ts.CLOUDFLARE_WORKERS_AI_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"cloudflare-workers-ai",
);

export const CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS: typeof ts.CLOUDFLARE_WORKERS_AI_CLASSIFIER_MODELS =
	wasm.aiBuiltinCatalog("classifier", "cloudflare-workers-ai");
