// models: createProvider and createModels over Rust providers. The slice bridges static providers with JS auth objects
// (api-key and OAuth), Rust API implementations or JS ones, JS credential and models stores and auth contexts, and
// login/logout; the methods marked notBridged belong to later slices.

import type {
	Api,
	AssistantMessage,
	AssistantMessageEventStream,
	Context,
	Credential,
	Model,
	ProviderAuth,
	ProviderStreams,
} from "@at-inc/pi-ai";
import type * as ts from "@at-inc/pi-ai/models";
import { streamFrom, wasm } from "@at-inc/pi-core";
import type { ModelsHandle, ProviderHandle } from "@at-inc/pi-core/wasm";
import { nativeOf } from "./internal/native-api.ts";
import { notBridged } from "./internal/not-bridged.ts";

export type * from "@at-inc/pi-ai/models";

const HANDLE = Symbol("pi-core ProviderHandle");

/** A provider created by createProvider: a Rust provider and the JS auth object it was created with. */
class RustProvider implements ts.Provider {
	readonly id: string;
	readonly name: string;
	readonly baseUrl?: string;
	readonly headers?: ts.Provider["headers"];
	readonly auth: ProviderAuth;
	readonly [HANDLE]: ProviderHandle;

	constructor(handle: ProviderHandle, auth: ProviderAuth) {
		const fields = handle.fields();
		this.id = fields.id;
		this.name = fields.name;
		if (fields.baseUrl !== undefined) this.baseUrl = fields.baseUrl;
		if (fields.headers !== undefined) this.headers = fields.headers;
		this.auth = auth;
		this[HANDLE] = handle;
	}

	getModels(): readonly Model<Api>[] {
		return this[HANDLE].getModels();
	}

	stream(...[model, context, options]: Parameters<ts.Provider["stream"]>): AssistantMessageEventStream {
		return streamFrom((sink) => this[HANDLE].stream(model, context, options, sink));
	}

	streamSimple(...[model, context, options]: Parameters<ts.Provider["streamSimple"]>): AssistantMessageEventStream {
		return streamFrom((sink) => this[HANDLE].streamSimple(model, context, options, sink));
	}
}

/** createProvider over a Rust provider; the bridged inputs are listed in the file header. */
export function createProvider<TApi extends Api = Api>(input: ts.CreateProviderOptions<TApi>): ts.Provider<TApi> {
	const api = input.api as ProviderStreams | Partial<Record<string, ProviderStreams>> | undefined;
	const single = api && typeof (api as ProviderStreams).stream === "function" ? (api as ProviderStreams) : undefined;
	const byApi = single || !api ? [] : Object.entries(api as Record<string, ProviderStreams | undefined>);
	const entries = byApi.filter((entry): entry is [string, ProviderStreams] => entry[1] !== undefined);
	if (input.images !== undefined || input.classifiers !== undefined)
		notBridged("createProvider({ images, classifiers })");
	if (input.fetchModels || input.filterModels || input.filterAllModels) {
		notBridged("createProvider({ fetchModels, filterModels, filterAllModels })");
	}
	const handle = wasm.aiCreateProvider(
		input,
		single,
		nativeOf(single)?.share(),
		entries.map(([apiName]) => apiName),
		entries.map(([, streams]) => streams),
		entries.map(([, streams]) => nativeOf(streams)?.share() ?? wasm.NativeStreams.placeholder()),
	);
	// RustProvider dispatches by the model's api at run time, so it serves the provider of any `TApi`.
	return new RustProvider(handle, input.auth) as unknown as ts.Provider<TApi>;
}

/** A Models collection over a Rust MutableModels; providers are kept here too, so getProvider returns the same object. */
class RustModels implements ts.MutableModels {
	readonly #handle: ModelsHandle;
	readonly #providers = new Map<string, ts.Provider>();

	constructor(options?: ts.CreateModelsOptions) {
		this.#handle = wasm.aiCreateModels(options ?? {});
	}

	setProvider(provider: ts.Provider): void {
		const handle = (provider as Partial<RustProvider>)[HANDLE];
		if (!handle) notBridged("Models.setProvider of a provider not made by createProvider");
		this.#handle.setProvider(handle.share());
		this.#providers.set(provider.id, provider);
	}

	deleteProvider(id: string): void {
		this.#handle.deleteProvider(id);
		this.#providers.delete(id);
	}

	clearProviders(): void {
		this.#handle.clearProviders();
		this.#providers.clear();
	}

	getProviders(): readonly ts.Provider[] {
		return Array.from(this.#providers.values());
	}

	getProvider(id: string): ts.Provider | undefined {
		return this.#providers.get(id);
	}

	getModels(provider?: string): readonly Model<Api>[] {
		return this.#handle.getModels(provider);
	}

	getModel(provider: string, id: string): Model<Api> | undefined {
		return this.#handle.getModel(provider, id);
	}

	stream<TApi extends Api>(
		model: Model<TApi>,
		context: Context,
		options?: ts.ModelsApiStreamOptions<TApi>,
	): AssistantMessageEventStream {
		return streamFrom((sink) => this.#handle.stream(model, context, options, sink));
	}

	async complete<TApi extends Api>(
		model: Model<TApi>,
		context: Context,
		options?: ts.ModelsApiStreamOptions<TApi>,
	): Promise<AssistantMessage> {
		return this.stream(model, context, options).result();
	}

	streamSimple(...[model, context, options]: Parameters<ts.Models["streamSimple"]>): AssistantMessageEventStream {
		return streamFrom((sink) => this.#handle.streamSimple(model, context, options, sink));
	}

	async completeSimple(...args: Parameters<ts.Models["completeSimple"]>): Promise<AssistantMessage> {
		return this.streamSimple(...args).result();
	}

	getModelsOfType(): never {
		return notBridged("Models.getModelsOfType");
	}

	getModelOfType(): never {
		return notBridged("Models.getModelOfType");
	}

	getAllModels(): never {
		return notBridged("Models.getAllModels");
	}

	refresh(): never {
		return notBridged("Models.refresh");
	}

	checkAuth(): never {
		return notBridged("Models.checkAuth");
	}

	getAvailable(): never {
		return notBridged("Models.getAvailable");
	}

	getAvailableOfType(): never {
		return notBridged("Models.getAvailableOfType");
	}

	getAllAvailable(): never {
		return notBridged("Models.getAllAvailable");
	}

	getAuth(): never {
		return notBridged("Models.getAuth");
	}

	async login(...[providerId, type, interaction, options]: Parameters<ts.Models["login"]>): Promise<Credential> {
		return this.#handle.login(providerId, type, interaction, options);
	}

	async logout(...[providerId, options]: Parameters<ts.Models["logout"]>): Promise<void> {
		return this.#handle.logout(providerId, options);
	}

	streamDeferred(): never {
		return notBridged("Models.streamDeferred");
	}

	fetchDeferred(): never {
		return notBridged("Models.fetchDeferred");
	}

	cancelDeferred(): never {
		return notBridged("Models.cancelDeferred");
	}

	generateImages(): never {
		return notBridged("Models.generateImages");
	}

	classify(): never {
		return notBridged("Models.classify");
	}
}

export const createModels: typeof ts.createModels = (options) => new RustModels(options);
