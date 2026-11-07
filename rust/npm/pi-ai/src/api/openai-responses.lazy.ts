// api/openai-responses.lazy: the OpenAI Responses API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/openai-responses.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openai-responses.lazy";

export const openAIResponsesApi: typeof ts.openAIResponsesApi = () => nativeApi(wasm.aiOpenAIResponsesApi());
