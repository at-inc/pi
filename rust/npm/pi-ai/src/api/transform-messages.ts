// api/transform-messages: the message transforms the API implementations share (cross-model replay, unsupported
// images, orphaned tool calls, images in error tool results).

import type * as ts from "@at-inc/pi-ai/api/transform-messages";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/transform-messages";

export const splitErrorToolResultImages: typeof ts.splitErrorToolResultImages = (msg) =>
	wasm.aiSplitErrorToolResultImages(msg);

export const transformMessages: typeof ts.transformMessages = (messages, model, normalizeToolCallId) =>
	wasm.aiTransformMessages(messages, model, normalizeToolCallId);
