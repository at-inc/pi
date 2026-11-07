// providers/xiaomi-token-plan-sgp.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/xiaomi-token-plan-sgp.models";
import { wasm } from "@at-inc/pi-core";

export const XIAOMI_TOKEN_PLAN_SGP_MODELS: typeof ts.XIAOMI_TOKEN_PLAN_SGP_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"xiaomi-token-plan-sgp",
);

export const XIAOMI_TOKEN_PLAN_SGP_IMAGE_MODELS: typeof ts.XIAOMI_TOKEN_PLAN_SGP_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"xiaomi-token-plan-sgp",
);

export const XIAOMI_TOKEN_PLAN_SGP_CLASSIFIER_MODELS: typeof ts.XIAOMI_TOKEN_PLAN_SGP_CLASSIFIER_MODELS =
	wasm.aiBuiltinCatalog("classifier", "xiaomi-token-plan-sgp");
