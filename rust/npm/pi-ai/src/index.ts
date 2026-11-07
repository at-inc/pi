// The core entry: types from @at-inc/pi-ai, and the runtime values of the modules the slices provide.

export type * from "@at-inc/pi-ai";
export { lazyApi, lazyStream } from "./api/lazy.ts";
export { defaultProviderAuthContext } from "./auth/context.ts";
export { InMemoryCredentialStore } from "./auth/credential-store.ts";
export { envApiKeyAuth, lazyOAuth } from "./auth/helpers.ts";
export {
	AI_TELEMETRY_SCHEMA,
	calculateCost,
	clampThinkingLevel,
	createModels,
	createProvider,
	getModelType,
	getSupportedThinkingLevels,
	hasApi,
	isModelType,
	ModelsError,
	modelsAreEqual,
} from "./models.ts";
export { InMemoryModelsStore } from "./models-store.ts";
export {
	createFauxCore,
	fauxAssistantMessage,
	fauxProvider,
	fauxText,
	fauxThinking,
	fauxToolCall,
} from "./providers/faux.ts";
export {
	appendAssistantMessageDiagnostic,
	createAssistantMessageDiagnostic,
	createProviderErrorDiagnostic,
	extractDiagnosticError,
	formatThrownValue,
} from "./utils/diagnostics.ts";
export { AssistantMessageEventStream, createAssistantMessageEventStream, EventStream } from "./utils/event-stream.ts";
export {
	formatUserAgent,
	getAttributionHeaders,
	getClientVersion,
	getIdentityHeaders,
	getOpenCodeSessionHeaders,
	mergeClientIdentity,
	PI_ATTRIBUTION,
	withClientIdentity,
} from "./utils/identity.ts";
export { parseJsonWithRepair, parseStreamingJson, parseToolCallArguments, repairJson } from "./utils/json-parse.ts";
export { getOverflowPatterns, isContextOverflow, isRecoverableLength } from "./utils/overflow.ts";
export {
	DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
	isRetryableAssistantError,
	retryAssistantCall,
	retryDelayMs,
} from "./utils/retry.ts";
export { StreamingJsonParser, ToolCallArgumentsStream } from "./utils/streaming-json.ts";
export { StringEnum } from "./utils/string-enum.ts";
export { contentText, getSystemMessageText, renderSystemMessageUpdate } from "./utils/text.ts";
export {
	collapseSystemMessages,
	createInitialSystemMessage,
	declarationsEqual,
	getCurrentSystemMessage,
	getCurrentSystemPrompt,
	getCurrentTools,
	getDeclaredTools,
	getInitialSystemMessage,
	getToolStateChanges,
	hasNonAdditiveToolChanges,
	hasToolRedefinitions,
	normalizeContext,
	resolveTranscript,
	resolveTranscriptTools,
	toToolDeclaration,
	withoutInitialSystemMessage,
} from "./utils/transcript.ts";
export { uuidv7 } from "./utils/uuid.ts";
export { validateToolArguments, validateToolCall } from "./utils/validation.ts";
export { PI_AI_VERSION } from "./utils/version.ts";
