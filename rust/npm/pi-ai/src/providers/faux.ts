// providers/faux: the message builders in the Rust core. The faux provider itself (createFauxCore, fauxProvider)
// belongs to the pi-ai models slice.

import type * as ts from "@at-inc/pi-ai/providers/faux";
import { wasm } from "@at-inc/pi-core";
import { notBridged } from "../internal/not-bridged.ts";

export type * from "@at-inc/pi-ai/providers/faux";

export const fauxText: typeof ts.fauxText = (text) => wasm.aiFauxText(text);

export const fauxThinking: typeof ts.fauxThinking = (thinking) => wasm.aiFauxThinking(thinking);

export const fauxToolCall: typeof ts.fauxToolCall = (name, arguments_, options) =>
	wasm.aiFauxToolCall(name, arguments_, options);

export const fauxAssistantMessage: typeof ts.fauxAssistantMessage = (content, options) =>
	wasm.aiFauxAssistantMessage(content, options);

export const createFauxCore: typeof ts.createFauxCore = () => notBridged("createFauxCore");

export const fauxProvider: typeof ts.fauxProvider = () => notBridged("fauxProvider");
