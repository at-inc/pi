// api/pi-messages: the pi messages API implementation.

import type * as ts from "@at-inc/pi-ai/api/pi-messages";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/pi-messages";

const streams = wasm.aiPiMessagesStreams();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => streams.stream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));
