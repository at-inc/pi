// providers/amazon-bedrock.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/amazon-bedrock.models";
import { wasm } from "@at-inc/pi-core";

export const AMAZON_BEDROCK_MODELS: typeof ts.AMAZON_BEDROCK_MODELS = wasm.aiBuiltinCatalog("chat", "amazon-bedrock");

export const AMAZON_BEDROCK_IMAGE_MODELS: typeof ts.AMAZON_BEDROCK_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"amazon-bedrock",
);

export const AMAZON_BEDROCK_CLASSIFIER_MODELS: typeof ts.AMAZON_BEDROCK_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"amazon-bedrock",
);
