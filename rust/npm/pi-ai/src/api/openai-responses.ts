// api/openai-responses: the OpenAI Responses API implementation.

import type * as ts from "@at-inc/pi-ai/api/openai-responses";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/openai-responses";

const streams = wasm.aiOpenAIResponsesStreams();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => streams.stream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));
