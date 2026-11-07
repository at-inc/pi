// api/google-generative-ai: the Google Generative AI API implementation.

import type * as ts from "@at-inc/pi-ai/api/google-generative-ai";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/google-generative-ai";

const streams = wasm.aiGoogleGenerativeAIStreams();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => streams.stream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));
