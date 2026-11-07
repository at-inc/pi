// providers/qwen-token-plan-individual.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/qwen-token-plan-individual.models";
import { wasm } from "@at-inc/pi-core";

export const QWEN_TOKEN_PLAN_INDIVIDUAL_MODELS: typeof ts.QWEN_TOKEN_PLAN_INDIVIDUAL_MODELS = wasm.aiBuiltinCatalog(
	"chat",
	"qwen-token-plan-individual",
);

export const QWEN_TOKEN_PLAN_INDIVIDUAL_IMAGE_MODELS: typeof ts.QWEN_TOKEN_PLAN_INDIVIDUAL_IMAGE_MODELS =
	wasm.aiBuiltinCatalog("image", "qwen-token-plan-individual");

export const QWEN_TOKEN_PLAN_INDIVIDUAL_CLASSIFIER_MODELS: typeof ts.QWEN_TOKEN_PLAN_INDIVIDUAL_CLASSIFIER_MODELS =
	wasm.aiBuiltinCatalog("classifier", "qwen-token-plan-individual");
