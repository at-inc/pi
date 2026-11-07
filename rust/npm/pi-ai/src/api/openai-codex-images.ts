// api/openai-codex-images: `generateImages` over the Rust API module.

import type * as ts from "@at-inc/pi-ai/api/openai-codex-images";
import { wasm } from "@at-inc/pi-core";
import { nativeImages } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openai-codex-images";

const api = nativeImages(wasm.aiOpenAICodexImagesApi());

export const generateImages: typeof ts.generateImages = (model, context, options) =>
	api.generateImages(model, context, options);
