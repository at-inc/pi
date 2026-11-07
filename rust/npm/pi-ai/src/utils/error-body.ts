// utils/error-body: provider error normalization in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/error-body";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/error-body";

export const MAX_PROVIDER_ERROR_BODY_CHARS =
	wasm.aiMaxProviderErrorBodyChars() as typeof ts.MAX_PROVIDER_ERROR_BODY_CHARS;

export const normalizeProviderError: typeof ts.normalizeProviderError = (error) => wasm.aiNormalizeProviderError(error);

export const formatProviderError: typeof ts.formatProviderError = (norm, prefix) =>
	wasm.aiFormatProviderError(norm, prefix);

export const truncateErrorText: typeof ts.truncateErrorText = (text, maxChars) =>
	wasm.aiTruncateErrorText(text, maxChars);

export const safeJsonStringify: typeof ts.safeJsonStringify = (value) => wasm.aiSafeJsonStringify(value);
