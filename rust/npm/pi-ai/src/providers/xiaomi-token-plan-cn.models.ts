// providers/xiaomi-token-plan-cn.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/xiaomi-token-plan-cn.models";
import { wasm } from "@at-inc/pi-core";

export const XIAOMI_TOKEN_PLAN_CN_MODELS: typeof ts.XIAOMI_TOKEN_PLAN_CN_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"xiaomi-token-plan-cn",
);

export const XIAOMI_TOKEN_PLAN_CN_IMAGE_MODELS: typeof ts.XIAOMI_TOKEN_PLAN_CN_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"xiaomi-token-plan-cn",
);

export const XIAOMI_TOKEN_PLAN_CN_CLASSIFIER_MODELS: typeof ts.XIAOMI_TOKEN_PLAN_CN_CLASSIFIER_MODELS =
	wasm.aiBuiltinCatalog("classifier", "xiaomi-token-plan-cn");
