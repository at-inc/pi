// providers/opencode-go.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/opencode-go.models";
import { wasm } from "@at-inc/pi-core";

export const OPENCODE_GO_MODELS: typeof ts.OPENCODE_GO_MODELS = wasm.aiBuiltinCatalog("chat", "opencode-go");

export const OPENCODE_GO_IMAGE_MODELS: typeof ts.OPENCODE_GO_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"opencode-go",
);

export const OPENCODE_GO_CLASSIFIER_MODELS: typeof ts.OPENCODE_GO_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"opencode-go",
);
