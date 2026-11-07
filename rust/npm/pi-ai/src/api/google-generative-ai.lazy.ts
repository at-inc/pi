// api/google-generative-ai.lazy: the Google Generative AI API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/google-generative-ai.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/google-generative-ai.lazy";

export const googleGenerativeAIApi: typeof ts.googleGenerativeAIApi = () => nativeApi(wasm.aiGoogleGenerativeAIApi());
