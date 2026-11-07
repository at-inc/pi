// providers/typesafe.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/typesafe.models";
import { wasm } from "@at-inc/pi-core";

export const TYPESAFE_MODELS: typeof ts.TYPESAFE_MODELS = wasm.aiBuiltinCatalog("chat", "typesafe");

export const TYPESAFE_IMAGE_MODELS: typeof ts.TYPESAFE_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "typesafe");

export const TYPESAFE_CLASSIFIER_MODELS: typeof ts.TYPESAFE_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog(
	"classifier",
	"typesafe",
);
