// models: createProvider and createModels over Rust providers, plus the module's helper functions. Providers written in
// JS (any `Provider` object) and the facade's Rust providers both go into a Rust `Models`, which calls the JS stores
// and auth context it is given; `login`/`logout` belong to the auth facade.

import type {
	AnyModel,
	Api,
	ApiKeyAuth,
	AssistantMessage,
	AssistantMessageEventStream,
	AuthCheck,
	AuthOperationOptions,
	AuthResult,
	Context,
	Model,
	ModelType,
	ModelTypeMap,
	ProviderAuth,
	ProviderClassifier,
	ProviderImages,
	ProviderStreams,
} from "@at-inc/pi-ai";
import type * as ts from "@at-inc/pi-ai/models";
import { streamFrom, wasm } from "@at-inc/pi-core";
import type { ApiKeyAuthHandle, ModelsHandle, ProviderHandle } from "@at-inc/pi-core/wasm";
import { rethrow, revived } from "./internal/errors.ts";
import { nativeClassifierOf, nativeImagesOf, nativeOf } from "./internal/native-api.ts";
import { notBridged } from "./internal/not-bridged.ts";

export type * from "@at-inc/pi-ai/models";
export { ModelsError } from "./utils/models-error.ts";

const HANDLE = Symbol("pi-core ProviderHandle");

/** A provider object over a Rust provider; `setProvider` hands its handle back to Rust. */
type RustProvider = ts.Provider & { readonly [HANDLE]: ProviderHandle };

/** The JS `Provider` of a Rust provider: its fields, `auth`, and the optional members the Rust provider has. */
export function providerFromHandle(handle: ProviderHandle, auth: ProviderAuth = authOf(handle)): ts.Provider {
	const fields = handle.fields();
	const provider: RustProvider = {
		id: fields.id,
		name: fields.name,
		baseUrl: fields.baseUrl,
		headers: fields.headers,
		auth,
		getModels: () => handle.getModels(),
		stream: (model, context, options) => streamFrom((sink) => handle.stream(model, context, options, sink)),
		streamSimple: (model, context, options) =>
			streamFrom((sink) => handle.streamSimple(model, context, options, sink)),
		[HANDLE]: handle,
	};
	const members: Partial<ts.Provider> = {
		getAllModels: () => handle.getAllModels(),
		refreshModels: (context) => handle.refreshModels(context).catch(rethrow),
		filterModels: (models, credential) => handle.filterModels(models, credential),
		filterAllModels: (models, credential) => handle.filterAllModels(models, credential),
		fetchDeferred: (model, deferred, options) =>
			streamFrom((sink) => handle.fetchDeferred(model, deferred, options, sink)),
		cancelDeferred: (model, deferred, options) => handle.cancelDeferred(model, deferred, options).catch(rethrow),
		generateImages: (model, context, options) => handle.generateImages(model, context, options).catch(rethrow),
		classify: (model, context, options) => handle.classify(model, context, options).catch(rethrow),
	};
	for (const name of handle.members() as (keyof typeof members)[]) {
		Object.assign(provider, { [name]: members[name] });
	}
	return provider;
}

/** The JS view of a Rust provider's auth. `login` belongs to the auth facade. */
function authOf(handle: ProviderHandle): ProviderAuth {
	const auth: ProviderAuth = {};
	const apiKey = handle.apiKeyAuth();
	if (apiKey) auth.apiKey = apiKeyAuthOf(apiKey);
	const oauth = handle.oauthAuth();
	if (oauth) {
		auth.oauth = {
			name: oauth.name,
			...(oauth.isSubscription !== undefined && { isSubscription: oauth.isSubscription }),
			...(oauth.loginLabel !== undefined && { loginLabel: oauth.loginLabel }),
			login: () => notBridged("oauth.login"),
			refresh: (credential, signal) => oauth.refresh(credential, signal).catch(rethrow),
			toAuth: (credential) => oauth.toAuth(credential).catch(rethrow),
		};
	}
	return auth;
}

/** The JS `ApiKeyAuth` of a Rust one. `login` belongs to the auth facade. */
export function apiKeyAuthOf(apiKey: ApiKeyAuthHandle): ApiKeyAuth {
	return {
		name: apiKey.name,
		resolve: (input) => apiKey.resolve(input).catch(rethrow),
		...(apiKey.hasCheck() && { check: (input) => apiKey.check(input).catch(rethrow) }),
		...(apiKey.hasLogin() && { login: () => notBridged("apiKey.login") }),
	};
}

/** The entries of an implementation map, without the `undefined` ones. */
function entriesOf<T>(map: Partial<Record<string, T>> | undefined): [string, T][] {
	return Object.entries(map ?? {}).filter((entry): entry is [string, T] => entry[1] !== undefined);
}

export const createProvider: typeof ts.createProvider = <TApi extends Api = Api>(
	input: ts.CreateProviderOptions<TApi>,
) => {
	const api = input.api as ProviderStreams | Partial<Record<string, ProviderStreams>> | undefined;
	const single = api && typeof (api as ProviderStreams).stream === "function" ? (api as ProviderStreams) : undefined;
	const chat = single ? [] : entriesOf(api as Partial<Record<string, ProviderStreams>> | undefined);
	const images = entriesOf<ProviderImages>(input.images);
	const classifiers = entriesOf<ProviderClassifier>(input.classifiers);
	const handle = wasm.aiCreateProvider(
		input,
		single,
		nativeOf(single)?.share(),
		chat.map(([name]) => name),
		chat.map(([, streams]) => streams),
		chat.map(([, streams]) => nativeOf(streams)?.share() ?? wasm.NativeStreams.placeholder()),
		images.map(([name]) => name),
		images.map(([, value]) => value),
		images.map(([, value]) => nativeImagesOf(value)?.share() ?? wasm.NativeImages.placeholder()),
		classifiers.map(([name]) => name),
		classifiers.map(([, value]) => value),
		classifiers.map(([, value]) => nativeClassifierOf(value)?.share() ?? wasm.NativeClassifier.placeholder()),
	);
	// The provider dispatches by the model's api at run time, so it serves the provider of any `TApi`.
	return providerFromHandle(handle, input.auth) as unknown as ts.Provider<TApi>;
};

/** A Models collection over a Rust MutableModels; the provider objects are kept here too, so getProvider returns them. */
export class RustModels implements ts.MutableModels {
	readonly #handle: ModelsHandle;
	readonly #providers = new Map<string, ts.Provider>();

	constructor(handle: ModelsHandle) {
		this.#handle = handle;
		for (const provider of handle.providerHandles()) {
			const view = providerFromHandle(provider);
			this.#providers.set(view.id, view);
		}
	}

	setProvider(provider: ts.Provider): void {
		const handle = (provider as Partial<RustProvider>)[HANDLE];
		if (handle) this.#handle.setProvider(handle.share());
		else this.#handle.setJsProvider(provider);
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

	getModelsOfType<TType extends ModelType>(type: TType, provider?: string): readonly ModelTypeMap[TType][] {
		return this.#handle.getModelsOfType(type, provider);
	}

	getModelOfType<TType extends ModelType>(type: TType, provider: string, id: string): ModelTypeMap[TType] | undefined {
		return this.#handle.getModelOfType(type, provider, id);
	}

	getAllModels(provider?: string): readonly AnyModel[] {
		return this.#handle.getAllModels(provider);
	}

	async refresh(options?: ts.ModelsRefreshOptions): Promise<ts.ModelsRefreshResult> {
		const result: { aborted: boolean; errors: [string, Error][] } = await this.#handle
			.refresh(options ?? {})
			.catch(rethrow);
		const errors = result.errors.map(([id, error]): [string, Error] => [id, revived(error) as Error]);
		return { aborted: result.aborted, errors: new Map(errors) };
	}

	checkAuth(providerId: string, options?: AuthOperationOptions): Promise<AuthCheck | undefined> {
		return this.#handle.checkAuth(providerId, options).catch(rethrow);
	}

	getAvailable(providerId?: string, options?: AuthOperationOptions): Promise<readonly Model<Api>[]> {
		return this.#handle.getAvailable(providerId, options).catch(rethrow);
	}

	getAvailableOfType<TType extends ModelType>(
		type: TType,
		providerId?: string,
		options?: AuthOperationOptions,
	): Promise<readonly ModelTypeMap[TType][]> {
		return this.#handle.getAvailableOfType(type, providerId, options).catch(rethrow);
	}

	getAllAvailable(providerId?: string, options?: AuthOperationOptions): Promise<readonly AnyModel[]> {
		return this.#handle.getAllAvailable(providerId, options).catch(rethrow);
	}

	getAuth(
		target: string | AnyModel,
		overrides?: Parameters<ts.Models["getAuth"]>[1],
	): Promise<AuthResult | undefined> {
		return this.#handle.getAuth(target, overrides).catch(rethrow);
	}

	login(): never {
		return notBridged("Models.login");
	}

	logout(): never {
		return notBridged("Models.logout");
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

	streamDeferred(...[model, deferred, options]: Parameters<ts.Models["streamDeferred"]>): AssistantMessageEventStream {
		return streamFrom((sink) => this.#handle.streamDeferred(model, deferred, options, sink));
	}

	async fetchDeferred(...args: Parameters<ts.Models["fetchDeferred"]>): Promise<AssistantMessage> {
		return this.streamDeferred(...args).result();
	}

	cancelDeferred(...[model, deferred, options]: Parameters<ts.Models["cancelDeferred"]>): Promise<void> {
		return this.#handle.cancelDeferred(model, deferred, options).catch(rethrow);
	}

	generateImages(
		...[model, context, options]: Parameters<ts.Models["generateImages"]>
	): ReturnType<ts.Models["generateImages"]> {
		return this.#handle.generateImages(model, context, options).catch(rethrow);
	}

	classify(...[model, context, options]: Parameters<ts.Models["classify"]>): ReturnType<ts.Models["classify"]> {
		return this.#handle.classify(model, context, options).catch(rethrow);
	}
}

export const createModels: typeof ts.createModels = (options) => new RustModels(wasm.aiCreateModels(options ?? {}));

export const hasApi: typeof ts.hasApi = ((model: AnyModel, api: string) =>
	wasm.aiHasApi(model, api)) as typeof ts.hasApi;

export const calculateCost: typeof ts.calculateCost = (model, usage) =>
	Object.assign(usage.cost, wasm.aiCalculateCost(model, usage));

export const getSupportedThinkingLevels: typeof ts.getSupportedThinkingLevels = (model) =>
	wasm.aiGetSupportedThinkingLevels(model);

export const clampThinkingLevel: typeof ts.clampThinkingLevel = (model, level) =>
	wasm.aiClampThinkingLevel(model, level);

export const modelsAreEqual: typeof ts.modelsAreEqual = (a, b) => wasm.aiModelsAreEqual(a, b);

export const getModelType: typeof ts.getModelType = (model) => wasm.aiGetModelType(model) as ModelType;

export const isModelType: typeof ts.isModelType = ((model: AnyModel, type: ModelType) =>
	wasm.aiIsModelType(model, type)) as typeof ts.isModelType;

export const AI_TELEMETRY_SCHEMA: typeof ts.AI_TELEMETRY_SCHEMA = wasm.aiTelemetrySchema();
