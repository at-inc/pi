// api/google-vertex: the Google Vertex AI API implementation.

import type * as ts from "@at-inc/pi-ai/api/google-vertex";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/google-vertex";

const streams = wasm.aiGoogleVertexStreams();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => streams.stream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));
