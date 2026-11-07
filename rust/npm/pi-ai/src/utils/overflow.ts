// utils/overflow: context overflow checks in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/overflow";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/overflow";

export const isContextOverflow: typeof ts.isContextOverflow = (message, contextWindow) =>
	wasm.aiIsContextOverflow(message, contextWindow);

export const isRecoverableLength: typeof ts.isRecoverableLength = (message, desiredMaxOutput) =>
	wasm.aiIsRecoverableLength(message, desiredMaxOutput);

export const getOverflowPatterns: typeof ts.getOverflowPatterns = () => wasm.aiGetOverflowPatterns();
