// utils/retry: the agent retry loop in the Rust core; `produce` and the callbacks are JS functions it awaits.

import type * as ts from "@at-inc/pi-ai/utils/retry";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/retry";

export const DEFAULT_MAX_AGENT_RETRY_DELAY_MS =
	wasm.aiDefaultMaxAgentRetryDelayMs() as typeof ts.DEFAULT_MAX_AGENT_RETRY_DELAY_MS;

export const retryDelayMs: typeof ts.retryDelayMs = (policy, attempt) => wasm.aiRetryDelayMs(policy, attempt);

export const retryAssistantCall: typeof ts.retryAssistantCall = (produce, policy, signal, callbacks) =>
	wasm.aiRetryAssistantCall(produce, policy, signal, callbacks);

export const isRetryableAssistantError: typeof ts.isRetryableAssistantError = (message) =>
	wasm.aiIsRetryableAssistantError(message);
