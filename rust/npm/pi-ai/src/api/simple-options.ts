// api/simple-options: the option arithmetic in the Rust core. buildBaseOptions copies the caller's option values
// (signals, callbacks, fetch) as they are and computes samplingParams and maxTokens in Rust.

import type * as ts from "@at-inc/pi-ai/api/simple-options";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/simple-options";

export const clampMaxTokensToContext: typeof ts.clampMaxTokensToContext = (model, context, maxTokens) =>
	wasm.aiClampMaxTokensToContext(model, context, maxTokens);

export const resolveSamplingParams: typeof ts.resolveSamplingParams = (model, thinkingLevel, requestParams) =>
	wasm.aiResolveSamplingParams(model, thinkingLevel, requestParams);

export const buildBaseOptions: typeof ts.buildBaseOptions = (model, context, options, apiKey) => ({
	temperature: options?.temperature,
	samplingParams: resolveSamplingParams(model, options?.reasoning ?? "off", options?.samplingParams),
	maxTokens: clampMaxTokensToContext(model, context, options?.maxTokens ?? model.maxTokens),
	signal: options?.signal,
	telemetry: options?.telemetry,
	apiKey: apiKey || options?.apiKey,
	fetch: options?.fetch,
	transport: options?.transport,
	cacheRetention: options?.cacheRetention,
	sessionId: options?.sessionId,
	headers: options?.headers,
	identity: options?.identity,
	onPayload: options?.onPayload,
	onResponse: options?.onResponse,
	onProviderStreamEvent: options?.onProviderStreamEvent,
	timeoutMs: options?.timeoutMs,
	websocketConnectTimeoutMs: options?.websocketConnectTimeoutMs,
	maxRetries: options?.maxRetries,
	maxRetryDelayMs: options?.maxRetryDelayMs,
	metadata: options?.metadata,
	env: options?.env,
});

export const MIN_ANSWER_TOKENS = wasm.aiMinAnswerTokens() as typeof ts.MIN_ANSWER_TOKENS;

export const DEFAULT_THINKING_BUDGETS: typeof ts.DEFAULT_THINKING_BUDGETS = wasm.aiDefaultThinkingBudgets();

export const clampReasoning: typeof ts.clampReasoning = (effort) => wasm.aiClampReasoning(effort);

export const thinkingBudgetForLevel: typeof ts.thinkingBudgetForLevel = (reasoningLevel, customBudgets) =>
	wasm.aiThinkingBudgetForLevel(reasoningLevel, customBudgets);

export const clampThinkingBudgetToAnswerRoom: typeof ts.clampThinkingBudgetToAnswerRoom = (thinkingBudget, ceiling) =>
	wasm.aiClampThinkingBudgetToAnswerRoom(thinkingBudget, ceiling);

export const adjustMaxTokensForThinking: typeof ts.adjustMaxTokensForThinking = (
	baseMaxTokens,
	modelMaxTokens,
	reasoningLevel,
	customBudgets,
) => wasm.aiAdjustMaxTokensForThinking(baseMaxTokens, modelMaxTokens, reasoningLevel, customBudgets);
