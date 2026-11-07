// providers/nvidia.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/nvidia.models";
import { wasm } from "@at-inc/pi-core";

export const NVIDIA_MODELS: typeof ts.NVIDIA_MODELS = wasm.aiBuiltinCatalog("chat", "nvidia");

export const NVIDIA_IMAGE_MODELS: typeof ts.NVIDIA_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "nvidia");

export const NVIDIA_CLASSIFIER_MODELS: typeof ts.NVIDIA_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"nvidia",
);
