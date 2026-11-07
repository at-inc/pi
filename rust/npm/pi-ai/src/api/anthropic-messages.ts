// api/anthropic-messages: the Anthropic Messages API implementation.

import type * as ts from "@at-inc/pi-ai/api/anthropic-messages";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/anthropic-messages";

const streams = wasm.aiAnthropicMessagesStreams();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => wasm.aiAnthropicMessagesStream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));
