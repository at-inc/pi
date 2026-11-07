// utils/text: text helpers in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/text";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/text";

export const contentText: typeof ts.contentText = (content, separator) => wasm.aiContentText(content, separator);

export const getSystemMessageText: typeof ts.getSystemMessageText = (message) => wasm.aiGetSystemMessageText(message);

export const renderSystemMessageUpdate: typeof ts.renderSystemMessageUpdate = (message) =>
	wasm.aiRenderSystemMessageUpdate(message);

export const ownedSlice: typeof ts.ownedSlice = (text, start, end) => wasm.aiOwnedSlice(text, start, end);
