// api/google-shared: shared utilities of the Google Generative AI and Google Vertex APIs.

import type * as ts from "@at-inc/pi-ai/api/google-shared";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/google-shared";

export const resolveGoogleThinkingLevel: typeof ts.resolveGoogleThinkingLevel = (model, level) =>
	wasm.aiGoogleSharedResolveGoogleThinkingLevel(model, level);

export const usesGoogleThinkingLevel: typeof ts.usesGoogleThinkingLevel = (model) =>
	wasm.aiGoogleSharedUsesGoogleThinkingLevel(model);

export const toGoogleThinkingLevel: typeof ts.toGoogleThinkingLevel = (level) =>
	wasm.aiGoogleSharedToGoogleThinkingLevel(level);

export const toGoogleSdkThinkingLevel: typeof ts.toGoogleSdkThinkingLevel = (level) =>
	wasm.aiGoogleSharedToGoogleSdkThinkingLevel(level);

export const getDisabledGoogleThinkingConfig: typeof ts.getDisabledGoogleThinkingConfig = (model) =>
	wasm.aiGoogleSharedGetDisabledGoogleThinkingConfig(model);

export const isThinkingPart: typeof ts.isThinkingPart = (part) => wasm.aiGoogleSharedIsThinkingPart(part);

export const retainThoughtSignature: typeof ts.retainThoughtSignature = (existing, incoming) =>
	wasm.aiGoogleSharedRetainThoughtSignature(existing, incoming);

export const requiresToolCallId: typeof ts.requiresToolCallId = (modelId) =>
	wasm.aiGoogleSharedRequiresToolCallId(modelId);

export const convertMessages: typeof ts.convertMessages = (model, context) =>
	wasm.aiGoogleSharedConvertMessages(model, context);

export const convertTools: typeof ts.convertTools = (tools, useParameters, supportsStrictMode) =>
	wasm.aiGoogleSharedConvertTools(tools, useParameters, supportsStrictMode);

export const supportsGoogleStrictToolSampling: typeof ts.supportsGoogleStrictToolSampling = (modelId) =>
	wasm.aiGoogleSharedSupportsGoogleStrictToolSampling(modelId);

export const mapToolChoice: typeof ts.mapToolChoice = (choice) =>
	wasm.aiGoogleSharedMapToolChoice(choice) as ReturnType<typeof ts.mapToolChoice>;

export const resolveGoogleFunctionCallingMode: typeof ts.resolveGoogleFunctionCallingMode = (
	tools,
	toolChoice,
	supportsStrictMode,
) =>
	wasm.aiGoogleSharedResolveGoogleFunctionCallingMode(tools, toolChoice, supportsStrictMode) as ReturnType<
		typeof ts.resolveGoogleFunctionCallingMode
	>;

export const mapStopReason: typeof ts.mapStopReason = (reason) => wasm.aiGoogleSharedMapStopReason(reason);

export const mapStopReasonString: typeof ts.mapStopReasonString = (reason) =>
	wasm.aiGoogleSharedMapStopReasonString(reason);

export const retryGoogleRequest: typeof ts.retryGoogleRequest = (request, options) =>
	wasm.aiGoogleSharedRetryGoogleRequest(request, options);
