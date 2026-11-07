// providers/qwen-token-plan.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/qwen-token-plan.models";
import { wasm } from "@at-inc/pi-core";

export const QWEN_TOKEN_PLAN_MODELS: typeof ts.QWEN_TOKEN_PLAN_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"qwen-token-plan",
);

export const QWEN_TOKEN_PLAN_IMAGE_MODELS: typeof ts.QWEN_TOKEN_PLAN_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"qwen-token-plan",
);

export const QWEN_TOKEN_PLAN_CLASSIFIER_MODELS: typeof ts.QWEN_TOKEN_PLAN_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"qwen-token-plan",
);
