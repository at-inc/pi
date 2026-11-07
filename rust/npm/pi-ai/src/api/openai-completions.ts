// api/openai-completions: the OpenAI Chat Completions API implementation.

import type * as ts from "@at-inc/pi-ai/api/openai-completions";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/openai-completions";

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => wasm.aiOpenAICompletionsStream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => wasm.aiOpenAICompletionsStreamSimple(model, context, options, sink));

export const convertMessages: typeof ts.convertMessages = (model, context, compat) =>
	wasm.aiOpenAICompletionsConvertMessages(model, context, compat);
