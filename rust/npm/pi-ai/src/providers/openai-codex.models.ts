// providers/openai-codex.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/openai-codex.models";
import { wasm } from "@at-inc/pi-core";

export const OPENAI_CODEX_MODELS: typeof ts.OPENAI_CODEX_MODELS = wasm.aiBuiltinCatalog("chat", "openai-codex");

export const OPENAI_CODEX_IMAGE_MODELS: typeof ts.OPENAI_CODEX_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"openai-codex",
);

export const OPENAI_CODEX_CLASSIFIER_MODELS: typeof ts.OPENAI_CODEX_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"openai-codex",
);
