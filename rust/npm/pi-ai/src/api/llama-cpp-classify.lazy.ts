// api/llama-cpp-classify.lazy: the API as ProviderClassifier.

import type * as ts from "@at-inc/pi-ai/api/llama-cpp-classify.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeClassifier } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/llama-cpp-classify.lazy";

export const llamaCppClassifyApi: typeof ts.llamaCppClassifyApi = () => nativeClassifier(wasm.aiLlamaCppClassifyApi());
