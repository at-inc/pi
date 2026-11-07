// providers/cloudflare-ai-gateway.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/cloudflare-ai-gateway.models";
import { wasm } from "@at-inc/pi-core";

export const CLOUDFLARE_AI_GATEWAY_MODELS: typeof ts.CLOUDFLARE_AI_GATEWAY_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"cloudflare-ai-gateway",
);

export const CLOUDFLARE_AI_GATEWAY_IMAGE_MODELS: typeof ts.CLOUDFLARE_AI_GATEWAY_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"cloudflare-ai-gateway",
);

export const CLOUDFLARE_AI_GATEWAY_CLASSIFIER_MODELS: typeof ts.CLOUDFLARE_AI_GATEWAY_CLASSIFIER_MODELS =
	wasm.aiBuiltinCatalog("classifier", "cloudflare-ai-gateway");
