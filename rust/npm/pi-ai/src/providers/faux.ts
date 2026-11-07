// providers/faux: the faux provider over the Rust faux core. Response factories stay JS functions that the core calls,
// and `state` is a live view of the core's counters.

import type { Model } from "@at-inc/pi-ai";
import type * as ts from "@at-inc/pi-ai/providers/faux";
import { wasm } from "@at-inc/pi-core";
import type { FauxCoreHandle } from "@at-inc/pi-core/wasm";
import { providerFromHandle } from "../models.ts";

export type * from "@at-inc/pi-ai/providers/faux";

export const fauxText: typeof ts.fauxText = (text) => wasm.aiFauxText(text);

export const fauxThinking: typeof ts.fauxThinking = (thinking) => wasm.aiFauxThinking(thinking);

export const fauxToolCall: typeof ts.fauxToolCall = (name, arguments_, options) =>
	wasm.aiFauxToolCall(name, arguments_, options);

export const fauxAssistantMessage: typeof ts.fauxAssistantMessage = (content, options) =>
	wasm.aiFauxAssistantMessage(content, options);

/** The members `fauxProvider()`, `registerFauxProvider()` and `createFauxCore()` share, over one core handle. */
export function fauxCoreView(handle: FauxCoreHandle) {
	const models = handle.models() as [Model<string>, ...Model<string>[]];
	const state: ts.FauxProviderState = {
		get callCount() {
			return handle.callCount();
		},
		get deferredFetchCount() {
			return handle.deferredFetchCount();
		},
		get cancelledDeferred() {
			return handle.cancelledDeferred();
		},
	};
	const steps = (responses: ts.FauxResponseStep[]) =>
		responses.map((step) =>
			typeof step === "function"
				? (context: unknown, options: unknown, model: unknown) =>
						step(context as never, options as never, state, model as never)
				: step,
		);
	function getModel(): Model<string>;
	function getModel(modelId: string): Model<string> | undefined;
	function getModel(modelId?: string): Model<string> | undefined {
		return modelId ? models.find((model) => model.id === modelId) : models[0];
	}
	return {
		api: handle.api,
		models,
		getModel,
		state,
		setResponses: (responses: ts.FauxResponseStep[]) => handle.setResponses(steps(responses)),
		appendResponses: (responses: ts.FauxResponseStep[]) => handle.appendResponses(steps(responses)),
		getPendingResponseCount: () => handle.getPendingResponseCount(),
	};
}

export const createFauxCore: typeof ts.createFauxCore = (options) => {
	const handle = wasm.aiFauxProvider(options);
	const provider = providerFromHandle(handle.provider()!);
	return {
		...fauxCoreView(handle),
		provider: provider.id,
		stream: provider.streamSimple,
		streamSimple: provider.streamSimple,
		fetchDeferred: provider.fetchDeferred!,
		cancelDeferred: provider.cancelDeferred!,
	};
};

export const fauxProvider: typeof ts.fauxProvider = (options = {}) => {
	const handle = wasm.aiFauxProvider(options);
	return { ...fauxCoreView(handle), provider: providerFromHandle(handle.provider()!) };
};
