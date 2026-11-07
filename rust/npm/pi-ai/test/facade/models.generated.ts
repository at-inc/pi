// The generated catalogs some TS tests import directly. `models.generated` is not a package entry, so the facade has no
// module for it; this shim reads the same catalogs from the Rust core.

import "./bridges.ts";
import { wasm } from "@at-inc/pi-core";

export const MODELS = wasm.aiBuiltinCatalogs("chat");

export const IMAGE_MODELS = wasm.aiBuiltinCatalogs("image");

export const CLASSIFIER_MODELS = wasm.aiBuiltinCatalogs("classifier");
