// api/bedrock-converse-stream: the Amazon Bedrock Converse Stream API implementation.

import type * as ts from "@at-inc/pi-ai/api/bedrock-converse-stream";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/bedrock-converse-stream";

const streams = wasm.aiBedrockConverseStreamStreams();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => streams.stream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));
