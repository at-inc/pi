// api/openai-completions.lazy: the OpenAI Chat Completions API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/openai-completions.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openai-completions.lazy";

export const openAICompletionsApi: typeof ts.openAICompletionsApi = () => nativeApi(wasm.aiOpenAICompletionsApi());
