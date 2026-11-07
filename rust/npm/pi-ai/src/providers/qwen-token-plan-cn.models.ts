// providers/qwen-token-plan-cn.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/qwen-token-plan-cn.models";
import { wasm } from "@at-inc/pi-core";

export const QWEN_TOKEN_PLAN_CN_MODELS: typeof ts.QWEN_TOKEN_PLAN_CN_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"qwen-token-plan-cn",
);

export const QWEN_TOKEN_PLAN_CN_IMAGE_MODELS: typeof ts.QWEN_TOKEN_PLAN_CN_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"qwen-token-plan-cn",
);

export const QWEN_TOKEN_PLAN_CN_CLASSIFIER_MODELS: typeof ts.QWEN_TOKEN_PLAN_CN_CLASSIFIER_MODELS =
	wasm.aiBuiltinCatalog("classifier", "qwen-token-plan-cn");
