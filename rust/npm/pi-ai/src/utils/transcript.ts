// utils/transcript: transcript helpers in the Rust core. Only system messages are read in Rust; the results hold the caller's
// other message objects.

import type * as ts from "@at-inc/pi-ai/utils/transcript";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/transcript";

export const createInitialSystemMessage: typeof ts.createInitialSystemMessage = (systemPrompt, tools) =>
	wasm.aiCreateInitialSystemMessage(systemPrompt, tools);

export const normalizeContext: typeof ts.normalizeContext = (context) => wasm.aiNormalizeContext(context);

export const getInitialSystemMessage: typeof ts.getInitialSystemMessage = (messages) =>
	wasm.aiGetInitialSystemMessage(messages);

export const withoutInitialSystemMessage: typeof ts.withoutInitialSystemMessage = (messages) =>
	wasm.aiWithoutInitialSystemMessage(messages);

export const getCurrentTools: typeof ts.getCurrentTools = (messages) => wasm.aiGetCurrentTools(messages);

export const getCurrentSystemMessage: typeof ts.getCurrentSystemMessage = (messages) =>
	wasm.aiGetCurrentSystemMessage(messages);

export const getCurrentSystemPrompt: typeof ts.getCurrentSystemPrompt = (messages) =>
	wasm.aiGetCurrentSystemPrompt(messages);

export const collapseSystemMessages: typeof ts.collapseSystemMessages = (context) =>
	wasm.aiCollapseSystemMessages(context);

export const resolveTranscript: typeof ts.resolveTranscript = (context, supportsMidConvoSystemMessages) =>
	wasm.aiResolveTranscript(context, supportsMidConvoSystemMessages);

export const toToolDeclaration: typeof ts.toToolDeclaration = (tool) => wasm.aiToToolDeclaration(tool);

export const declarationsEqual: typeof ts.declarationsEqual = (left, right) => wasm.aiDeclarationsEqual(left, right);

export const getToolStateChanges: typeof ts.getToolStateChanges = (previous, current) =>
	wasm.aiGetToolStateChanges(previous, current);

export const getDeclaredTools: typeof ts.getDeclaredTools = (messages) => wasm.aiGetDeclaredTools(messages);

/** @deprecated See the TS declaration. */
export const hasToolRedefinitions: typeof ts.hasToolRedefinitions = (messages) => wasm.aiHasToolRedefinitions(messages);

export const hasNonAdditiveToolChanges: typeof ts.hasNonAdditiveToolChanges = (messages) =>
	wasm.aiHasNonAdditiveToolChanges(messages);

export const resolveTranscriptTools: typeof ts.resolveTranscriptTools = (messages, support) =>
	wasm.aiResolveTranscriptTools(messages, support);
