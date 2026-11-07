// api/openrouter-images.lazy: the API as ProviderImages.

import type * as ts from "@at-inc/pi-ai/api/openrouter-images.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeImages } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openrouter-images.lazy";

export const openrouterImagesApi: typeof ts.openrouterImagesApi = () => nativeImages(wasm.aiOpenrouterImagesApi());
