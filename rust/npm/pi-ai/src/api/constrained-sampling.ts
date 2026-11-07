// api/constrained-sampling: strict JSON-schema and grammar constrained sampling of tool calls.

import type * as ts from "@at-inc/pi-ai/api/constrained-sampling";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/constrained-sampling";

export const makeStrictJsonSchema: typeof ts.makeStrictJsonSchema = (schema, isUnsupportedKeyword) =>
	wasm.aiMakeStrictJsonSchema(schema, isUnsupportedKeyword);

export const prefersJsonSchemaStrictSampling: typeof ts.prefersJsonSchemaStrictSampling = (tool) =>
	wasm.aiPrefersJsonSchemaStrictSampling(tool);

export const isStrictToolLimitErrorMessage: typeof ts.isStrictToolLimitErrorMessage = (message) =>
	wasm.aiIsStrictToolLimitErrorMessage(message);

export const getJsonSchemaToolParameters: typeof ts.getJsonSchemaToolParameters = (tool, strict) =>
	wasm.aiGetJsonSchemaToolParameters(tool, strict);

export const getGrammarToolInput: typeof ts.getGrammarToolInput = (toolName, arguments_, inputProperty) =>
	wasm.aiGetGrammarToolInput(toolName, arguments_, inputProperty);

export const appendGrammarToolInputJsonDelta: typeof ts.appendGrammarToolInputJsonDelta = (
	buffer,
	inputProperty,
	inputDelta,
) => wasm.aiAppendGrammarToolInputJsonDelta(buffer, inputProperty, inputDelta);

export const closeGrammarToolInputJson: typeof ts.closeGrammarToolInputJson = (buffer, inputProperty, finalInput) =>
	wasm.aiCloseGrammarToolInputJson(buffer, inputProperty, finalInput);

export const resolveJsonSchemaStrictSampling: typeof ts.resolveJsonSchemaStrictSampling = (
	tool,
	supportsStrictMode,
	isUnsupportedKeyword,
) => wasm.aiResolveJsonSchemaStrictSampling(tool, supportsStrictMode, isUnsupportedKeyword);

export const resolveGrammarConstrainedSampling: typeof ts.resolveGrammarConstrainedSampling = (
	tool,
	supportsOpenAIGrammarTools,
) => wasm.aiResolveGrammarConstrainedSampling(tool, supportsOpenAIGrammarTools);

export const createGrammarToolInputProperties: typeof ts.createGrammarToolInputProperties = (
	tools,
	supportsOpenAIGrammarTools,
) => new Map(wasm.aiCreateGrammarToolInputProperties(tools, supportsOpenAIGrammarTools));
