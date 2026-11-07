// api/openai-responses-shared: message, tool and stream conversion shared by the OpenAI Responses APIs.

import type * as ts from "@at-inc/pi-ai/api/openai-responses-shared";
import { AssistantMessageEventSink, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/openai-responses-shared";

export const getResponsesToolSupport: typeof ts.getResponsesToolSupport = (options) =>
	wasm.aiGetResponsesToolSupport(options);

export const convertResponsesMessages: typeof ts.convertResponsesMessages = (
	model,
	context,
	allowedToolCallProviders,
	options,
) => wasm.aiConvertResponsesMessages(model, context, allowedToolCallProviders, options);

export const convertResponsesTools: typeof ts.convertResponsesTools = (tools, options) =>
	wasm.aiConvertResponsesTools(tools, options);

export const processResponsesStream: typeof ts.processResponsesStream = (
	openaiStream,
	output,
	stream,
	model,
	options,
) => wasm.aiProcessResponsesStream(openaiStream, output, model, options, new AssistantMessageEventSink(stream, output));
