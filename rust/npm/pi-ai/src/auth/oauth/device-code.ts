// auth/oauth/device-code: the RFC 8628 device-code polling loop.

import type * as ts from "@at-inc/pi-ai/auth/oauth/device-code";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/auth/oauth/device-code";

export const abortableSleep: typeof ts.abortableSleep = (ms, signal, cancelMessage) =>
	wasm.aiAbortableSleep(ms, signal, cancelMessage);

export const pollOAuthDeviceCodeFlow: typeof ts.pollOAuthDeviceCodeFlow = async (options) =>
	wasm.aiPollOAuthDeviceCodeFlow(options);
