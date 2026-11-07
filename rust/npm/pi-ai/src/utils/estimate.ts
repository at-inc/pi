// utils/estimate: token estimates in the Rust core; messages cross as JSON text.

import type * as ts from "@at-inc/pi-ai/utils/estimate";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/estimate";

export const calculateContextTokens: typeof ts.calculateContextTokens = (usage) => wasm.aiCalculateContextTokens(usage);

export const estimateTextTokens: typeof ts.estimateTextTokens = (text) => wasm.aiEstimateTextTokens(text);

export const estimateTextAndImageContentTokens: typeof ts.estimateTextAndImageContentTokens = (content) =>
	wasm.aiEstimateTextAndImageContentTokens(content);

export const estimateMessageTokens: typeof ts.estimateMessageTokens = (message) =>
	wasm.aiEstimateMessageTokens(message);

export const estimateContextTokens: typeof ts.estimateContextTokens = (context) =>
	wasm.aiEstimateContextTokens(context);
