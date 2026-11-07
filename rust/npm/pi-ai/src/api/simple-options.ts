// api/simple-options: the mapping from SimpleStreamOptions to an API's stream options (base options, max tokens,
// thinking budgets).

import type * as ts from "@at-inc/pi-ai/api/simple-options";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/simple-options";

export const clampMaxTokensToContext: typeof ts.clampMaxTokensToContext = (model, context, maxTokens) =>
	wasm.aiClampMaxTokensToContext(model, context, maxTokens);

export const resolveSamplingParams: typeof ts.resolveSamplingParams = (model, thinkingLevel, requestParams) =>
	wasm.aiResolveSamplingParams(model, thinkingLevel, requestParams);

// The fields that hold objects with identity are the caller's own, as in the TS; Rust computes the data fields.
export const buildBaseOptions: typeof ts.buildBaseOptions = (model, context, options, apiKey) => ({
	...wasm.aiBuildBaseOptions(model, context, options, apiKey),
	signal: options?.signal,
	telemetry: options?.telemetry,
	fetch: options?.fetch,
	onPayload: options?.onPayload,
	onResponse: options?.onResponse,
	onProviderStreamEvent: options?.onProviderStreamEvent,
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
