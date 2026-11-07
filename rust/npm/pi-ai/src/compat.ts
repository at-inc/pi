// compat: the old global API. The slice provides api-dispatch stream/complete with env API keys, getModel and the
// openai-completions API; the other per-API wrappers, the api-registry and images belong to later slices.

import type * as ts from "@at-inc/pi-ai/compat";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/compat";
export { anthropicMessagesApi } from "./api/anthropic-messages.lazy.ts";
export { azureOpenAIResponsesApi } from "./api/azure-openai-responses.lazy.ts";
export { bedrockConverseStreamApi } from "./api/bedrock-converse-stream.lazy.ts";
export { googleGenerativeAIApi } from "./api/google-generative-ai.lazy.ts";
export { googleVertexApi } from "./api/google-vertex.lazy.ts";
export { mistralConversationsApi } from "./api/mistral-conversations.lazy.ts";
export { openAICodexResponsesApi } from "./api/openai-codex-responses.lazy.ts";
export { openAICompletionsApi } from "./api/openai-completions.lazy.ts";
export { openAIResponsesApi } from "./api/openai-responses.lazy.ts";
export { piMessagesApi } from "./api/pi-messages.lazy.ts";
export {
	AssistantMessageEventStream,
	createAssistantMessageEventStream,
	createModels,
	createProvider,
	EventStream,
	normalizeContext,
} from "./index.ts";

export const getModel: typeof ts.getModel = ((provider: string, modelId: string) =>
	wasm.aiGetBuiltinModel(provider, modelId)) as typeof ts.getModel;

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => wasm.aiStream(model, context, options, sink));

export const complete: typeof ts.complete = async (model, context, options) => stream(model, context, options).result();

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => wasm.aiStreamSimple(model, context, options, sink));

export const completeSimple: typeof ts.completeSimple = async (model, context, options) =>
	streamSimple(model, context, options).result();
