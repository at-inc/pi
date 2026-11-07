// api/openai-codex-images.lazy: the API as ProviderImages.

import type * as ts from "@at-inc/pi-ai/api/openai-codex-images.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeImages } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openai-codex-images.lazy";

export const openAICodexImagesApi: typeof ts.openAICodexImagesApi = () => nativeImages(wasm.aiOpenAICodexImagesApi());
