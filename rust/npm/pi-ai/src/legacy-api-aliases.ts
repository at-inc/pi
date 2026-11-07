// legacy-api-aliases: the old per-API stream functions, over the lazy API implementations as in the TS.

import type * as ts from "@at-inc/pi-ai/compat";
import { anthropicMessagesApi } from "./api/anthropic-messages.lazy.ts";
import { azureOpenAIResponsesApi } from "./api/azure-openai-responses.lazy.ts";
import { googleGenerativeAIApi } from "./api/google-generative-ai.lazy.ts";
import { googleVertexApi } from "./api/google-vertex.lazy.ts";
import { mistralConversationsApi } from "./api/mistral-conversations.lazy.ts";
import { openAICodexResponsesApi } from "./api/openai-codex-responses.lazy.ts";
import { openAICompletionsApi } from "./api/openai-completions.lazy.ts";
import { openAIResponsesApi } from "./api/openai-responses.lazy.ts";

const anthropicMessagesStreams = anthropicMessagesApi();
const azureOpenAIResponsesStreams = azureOpenAIResponsesApi();
const googleGenerativeAIStreams = googleGenerativeAIApi();
const googleVertexStreams = googleVertexApi();
const mistralConversationsStreams = mistralConversationsApi();
const openAICodexResponsesStreams = openAICodexResponsesApi();
const openAICompletionsStreams = openAICompletionsApi();
const openAIResponsesStreams = openAIResponsesApi();

export const streamAnthropic: typeof ts.streamAnthropic = anthropicMessagesStreams.stream as typeof ts.streamAnthropic;

export const streamSimpleAnthropic: typeof ts.streamSimpleAnthropic =
	anthropicMessagesStreams.streamSimple as typeof ts.streamSimpleAnthropic;

export const streamAzureOpenAIResponses: typeof ts.streamAzureOpenAIResponses =
	azureOpenAIResponsesStreams.stream as typeof ts.streamAzureOpenAIResponses;

export const streamSimpleAzureOpenAIResponses: typeof ts.streamSimpleAzureOpenAIResponses =
	azureOpenAIResponsesStreams.streamSimple as typeof ts.streamSimpleAzureOpenAIResponses;

export const streamGoogle: typeof ts.streamGoogle = googleGenerativeAIStreams.stream as typeof ts.streamGoogle;

export const streamSimpleGoogle: typeof ts.streamSimpleGoogle =
	googleGenerativeAIStreams.streamSimple as typeof ts.streamSimpleGoogle;

export const streamGoogleVertex: typeof ts.streamGoogleVertex =
	googleVertexStreams.stream as typeof ts.streamGoogleVertex;

export const streamSimpleGoogleVertex: typeof ts.streamSimpleGoogleVertex =
	googleVertexStreams.streamSimple as typeof ts.streamSimpleGoogleVertex;

export const streamMistral: typeof ts.streamMistral = mistralConversationsStreams.stream as typeof ts.streamMistral;

export const streamSimpleMistral: typeof ts.streamSimpleMistral =
	mistralConversationsStreams.streamSimple as typeof ts.streamSimpleMistral;

export const streamOpenAICodexResponses: typeof ts.streamOpenAICodexResponses =
	openAICodexResponsesStreams.stream as typeof ts.streamOpenAICodexResponses;

export const streamSimpleOpenAICodexResponses: typeof ts.streamSimpleOpenAICodexResponses =
	openAICodexResponsesStreams.streamSimple as typeof ts.streamSimpleOpenAICodexResponses;

export const streamOpenAICompletions: typeof ts.streamOpenAICompletions =
	openAICompletionsStreams.stream as typeof ts.streamOpenAICompletions;

export const streamSimpleOpenAICompletions: typeof ts.streamSimpleOpenAICompletions =
	openAICompletionsStreams.streamSimple as typeof ts.streamSimpleOpenAICompletions;

export const streamOpenAIResponses: typeof ts.streamOpenAIResponses =
	openAIResponsesStreams.stream as typeof ts.streamOpenAIResponses;

export const streamSimpleOpenAIResponses: typeof ts.streamSimpleOpenAIResponses =
	openAIResponsesStreams.streamSimple as typeof ts.streamSimpleOpenAIResponses;
