// compat: the old global API over the Rust core: api-dispatch stream/complete with env API keys, the api-registry,
// the built-in catalog reads, the per-API lazy stream wrappers, and image generation. The registries live in Rust, so
// `getApiProvider` returns a new view object of the registered provider on each call.

import type * as ts from "@at-inc/pi-ai/compat";
import { streamFrom, wasm } from "@at-inc/pi-core";
import { getBuiltinModel, getBuiltinModels, getBuiltinProviders } from "./providers/all.ts";
import { fauxCoreView } from "./providers/faux.ts";

export type * from "@at-inc/pi-ai/compat";
export { anthropicMessagesApi } from "./api/anthropic-messages.lazy.ts";
export { azureOpenAIResponsesApi } from "./api/azure-openai-responses.lazy.ts";
export { bedrockConverseStreamApi, setBedrockProviderModule } from "./api/bedrock-converse-stream.lazy.ts";
export { googleGenerativeAIApi } from "./api/google-generative-ai.lazy.ts";
export { googleVertexApi } from "./api/google-vertex.lazy.ts";
export { mistralConversationsApi } from "./api/mistral-conversations.lazy.ts";
export { openAICodexResponsesApi } from "./api/openai-codex-responses.lazy.ts";
export { openAICompletionsApi } from "./api/openai-completions.lazy.ts";
export { openAIResponsesApi } from "./api/openai-responses.lazy.ts";
export { piMessagesApi } from "./api/pi-messages.lazy.ts";
export {
	ANTHROPIC_API_KEY_ENV,
	ANTHROPIC_AUTH_TOKEN_ENV,
	ANTHROPIC_FEDERATION_RULE_ID_ENV,
	ANTHROPIC_IDENTITY_TOKEN_FILE_ENV,
	ANTHROPIC_OAUTH_TOKEN_ENV,
	ANTHROPIC_ORGANIZATION_ID_ENV,
	ANTHROPIC_SERVICE_ACCOUNT_ID_ENV,
	ANTHROPIC_WORKSPACE_ID_ENV,
	findEnvKeys,
	getEnvApiKey,
} from "./env-api-keys.ts";
export { getImageModel, getImageModels, getImageProviders } from "./image-models.ts";
export { generateImages } from "./images.ts";
export { getImagesApiProvider, registerImagesApiProvider } from "./images-api-registry.ts";
export {
	AI_TELEMETRY_SCHEMA,
	AssistantMessageEventStream,
	appendAssistantMessageDiagnostic,
	calculateCost,
	clampThinkingLevel,
	cleanupSessionResources,
	collapseSystemMessages,
	contentText,
	createAssistantMessageDiagnostic,
	createAssistantMessageEventStream,
	createFauxCore,
	createInitialSystemMessage,
	createModels,
	createProvider,
	createProviderErrorDiagnostic,
	DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
	declarationsEqual,
	defaultProviderAuthContext,
	EventStream,
	envApiKeyAuth,
	extractDiagnosticError,
	fauxAssistantMessage,
	fauxProvider,
	fauxText,
	fauxThinking,
	fauxToolCall,
	formatThrownValue,
	formatUserAgent,
	getAttributionHeaders,
	getClientVersion,
	getCurrentSystemMessage,
	getCurrentSystemPrompt,
	getCurrentTools,
	getDeclaredTools,
	getIdentityHeaders,
	getInitialSystemMessage,
	getModelType,
	getOpenCodeSessionHeaders,
	getOverflowPatterns,
	getSupportedThinkingLevels,
	getSystemMessageText,
	getToolStateChanges,
	hasApi,
	hasNonAdditiveToolChanges,
	hasToolRedefinitions,
	InMemoryCredentialStore,
	InMemoryModelsStore,
	isContextOverflow,
	isModelType,
	isRecoverableLength,
	isRetryableAssistantError,
	lazyApi,
	lazyOAuth,
	lazyStream,
	ModelsError,
	mergeClientIdentity,
	modelsAreEqual,
	normalizeContext,
	PI_AI_VERSION,
	PI_ATTRIBUTION,
	parseJsonWithRepair,
	parseStreamingJson,
	parseToolCallArguments,
	registerSessionResourceCleanup,
	renderSystemMessageUpdate,
	repairJson,
	resolveTranscript,
	resolveTranscriptTools,
	retryAssistantCall,
	retryDelayMs,
	StreamingJsonParser,
	StringEnum,
	ToolCallArgumentsStream,
	toToolDeclaration,
	uuidv7,
	validateToolArguments,
	validateToolCall,
	withClientIdentity,
	withoutInitialSystemMessage,
} from "./index.ts";
export {
	streamAnthropic,
	streamAzureOpenAIResponses,
	streamGoogle,
	streamGoogleVertex,
	streamMistral,
	streamOpenAICodexResponses,
	streamOpenAICompletions,
	streamOpenAIResponses,
	streamSimpleAnthropic,
	streamSimpleAzureOpenAIResponses,
	streamSimpleGoogle,
	streamSimpleGoogleVertex,
	streamSimpleMistral,
	streamSimpleOpenAICodexResponses,
	streamSimpleOpenAICompletions,
	streamSimpleOpenAIResponses,
} from "./legacy-api-aliases.ts";
export {
	generateImagesOpenAICodex,
	generateImagesOpenRouter,
	registerBuiltInImagesApiProviders,
} from "./providers/images/register-builtins.ts";

/** @deprecated Static catalog read. Use `getBuiltinModel` from "@at-inc/pi-ai/providers/all" or `Models.getModel()`. */
export const getModel: typeof ts.getModel = getBuiltinModel;

/** @deprecated Static catalog read. Use `getBuiltinModels` from "@at-inc/pi-ai/providers/all" or `Models.getModels()`. */
export const getModels: typeof ts.getModels = getBuiltinModels;

/** @deprecated Static catalog read. Use `getBuiltinProviders` from "@at-inc/pi-ai/providers/all". */
export const getProviders: typeof ts.getProviders = getBuiltinProviders;

export const registerApiProvider: typeof ts.registerApiProvider = (provider, sourceId) =>
	wasm.aiRegisterApiProvider(provider, sourceId);

/** The JS view of a registered api provider. */
function apiProviderOf(provider: ReturnType<typeof wasm.aiGetApiProviders>[number]) {
	return {
		api: provider.api,
		stream: ((model, context, options) =>
			streamFrom((sink) => provider.stream(model, context, options, sink))) as ts.ApiStreamFunction,
		streamSimple: ((model, context, options) =>
			streamFrom((sink) => provider.streamSimple(model, context, options, sink))) as ts.ApiStreamSimpleFunction,
	};
}

export const getApiProvider: typeof ts.getApiProvider = (api) => {
	const provider = wasm.aiGetApiProvider(api);
	return provider && apiProviderOf(provider);
};

export const getApiProviders: typeof ts.getApiProviders = () => wasm.aiGetApiProviders().map(apiProviderOf);

export const unregisterApiProviders: typeof ts.unregisterApiProviders = (sourceId) =>
	wasm.aiUnregisterApiProviders(sourceId);

export const registerBuiltInApiProviders: typeof ts.registerBuiltInApiProviders = () =>
	wasm.aiRegisterBuiltInApiProviders();

export const registerFauxProvider: typeof ts.registerFauxProvider = (options = {}) => {
	const handle = wasm.aiRegisterFauxProvider(options);
	return { ...fauxCoreView(handle), unregister: () => handle.unregister() };
};

export const resetApiProviders: typeof ts.resetApiProviders = () => wasm.aiResetApiProviders();

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => wasm.aiStream(model, context, options, sink));

export const complete: typeof ts.complete = async (model, context, options) => stream(model, context, options).result();

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => wasm.aiStreamSimple(model, context, options, sink));

export const completeSimple: typeof ts.completeSimple = async (model, context, options) =>
	streamSimple(model, context, options).result();
