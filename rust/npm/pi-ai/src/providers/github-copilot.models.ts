// providers/github-copilot.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/github-copilot.models";
import { wasm } from "@at-inc/pi-core";

export const GITHUB_COPILOT_MODELS: typeof ts.GITHUB_COPILOT_MODELS = wasm.aiBuiltinCatalog("chat", "github-copilot");

export const GITHUB_COPILOT_IMAGE_MODELS: typeof ts.GITHUB_COPILOT_IMAGE_MODELS = wasm.aiBuiltinCatalog(
	"image",
	"github-copilot",
);

export const GITHUB_COPILOT_CLASSIFIER_MODELS: typeof ts.GITHUB_COPILOT_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"github-copilot",
);
