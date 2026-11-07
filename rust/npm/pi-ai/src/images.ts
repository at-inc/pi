// images: the global image generation, dispatched on `model.api` through the images api registry in the Rust core.

import type * as ts from "@at-inc/pi-ai/compat";
import { wasm } from "@at-inc/pi-core";
import { rethrow } from "./internal/errors.ts";

export const generateImages: typeof ts.generateImages = (model, context, options) =>
	wasm.aiGenerateImages(model, context, options).catch(rethrow);
