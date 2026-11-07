// api/openai-codex-responses.lazy: the OpenAI Codex Responses API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/openai-codex-responses.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openai-codex-responses.lazy";

export const openAICodexResponsesApi: typeof ts.openAICodexResponsesApi = () =>
	nativeApi(wasm.aiOpenAICodexResponsesApi());
