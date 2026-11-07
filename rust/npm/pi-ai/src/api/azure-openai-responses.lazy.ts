// api/azure-openai-responses.lazy: the Azure OpenAI Responses API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/azure-openai-responses.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/azure-openai-responses.lazy";

export const azureOpenAIResponsesApi: typeof ts.azureOpenAIResponsesApi = () =>
	nativeApi(wasm.aiAzureOpenAIResponsesApi());
