// providers/groq.models: the generated catalogs of the provider, read from the Rust core's catalog.

import type * as ts from "@at-inc/pi-ai/providers/groq.models";
import { wasm } from "@at-inc/pi-core";

export const GROQ_MODELS: typeof ts.GROQ_MODELS = wasm.aiBuiltinCatalog("chat", "groq");

export const GROQ_IMAGE_MODELS: typeof ts.GROQ_IMAGE_MODELS = wasm.aiBuiltinCatalog("image", "groq");

export const GROQ_CLASSIFIER_MODELS: typeof ts.GROQ_CLASSIFIER_MODELS = wasm.aiBuiltinCatalog("classifier", "groq");
