// api/github-copilot-headers: the per-request headers GitHub Copilot expects.

import type * as ts from "@at-inc/pi-ai/api/github-copilot-headers";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/github-copilot-headers";

export const inferCopilotInitiator: typeof ts.inferCopilotInitiator = (messages) =>
	wasm.aiInferCopilotInitiator(messages) as ReturnType<typeof ts.inferCopilotInitiator>;

export const hasCopilotVisionInput: typeof ts.hasCopilotVisionInput = (messages) =>
	wasm.aiHasCopilotVisionInput(messages);

export const buildCopilotDynamicHeaders: typeof ts.buildCopilotDynamicHeaders = (params) =>
	wasm.aiBuildCopilotDynamicHeaders(params);
