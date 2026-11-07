// providers/vercel-ai-gateway.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/vercel-ai-gateway.models";
import { wasm } from "@at-inc/pi-core";

export const VERCEL_AI_GATEWAY_MODELS: typeof ts.VERCEL_AI_GATEWAY_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"vercel-ai-gateway",
);

export const VERCEL_AI_GATEWAY_IMAGE_MODELS: typeof ts.VERCEL_AI_GATEWAY_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"vercel-ai-gateway",
);

export const VERCEL_AI_GATEWAY_CLASSIFIER_MODELS: typeof ts.VERCEL_AI_GATEWAY_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"vercel-ai-gateway",
);
