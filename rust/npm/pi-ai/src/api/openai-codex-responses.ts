// api/openai-codex-responses: the OpenAI Codex Responses API implementation.

import type * as ts from "@at-inc/pi-ai/api/openai-codex-responses";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/openai-codex-responses";

const streams = wasm.aiOpenAICodexResponsesStreams();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => streams.stream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));

export const getOpenAICodexWebSocketDebugStats: typeof ts.getOpenAICodexWebSocketDebugStats = (sessionId) =>
	wasm.aiGetOpenAICodexWebSocketDebugStats(sessionId);

export const resetOpenAICodexWebSocketDebugStats: typeof ts.resetOpenAICodexWebSocketDebugStats = (sessionId) =>
	wasm.aiResetOpenAICodexWebSocketDebugStats(sessionId);

export const closeOpenAICodexWebSocketSessions: typeof ts.closeOpenAICodexWebSocketSessions = (sessionId) =>
	wasm.aiCloseOpenAICodexWebSocketSessions(sessionId);
