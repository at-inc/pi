# @at-inc/pi-ai-rs

`@at-inc/pi-ai` backed by the pi Rust core. It has the same exports map and module layout, the types of
`@at-inc/pi-ai` (a peer dependency) and a runtime in wasm (`@at-inc/pi-core`). Switch to it with a package alias:
`"@at-inc/pi-ai": "npm:@at-inc/pi-ai-rs@<version>"`.

## Bridged

- `.`: `createProvider`, `createModels`, `normalizeContext`, `EventStream`, `AssistantMessageEventStream`,
  `createAssistantMessageEventStream`.
- `./compat`: the above, plus `getModel`, `stream`, `complete`, `streamSimple`, `completeSimple` (dispatch by
  `model.api` with env API keys) and `openAICompletionsApi`.
- `./api/openai-completions`: `stream`, `streamSimple`, `convertMessages`.
- `./api/openai-completions.lazy`: `openAICompletionsApi`.
- `./models`: `createProvider` (static providers with api-key auth, Rust or JS API implementations) and `createModels`
  (`setProvider`, `deleteProvider`, `clearProviders`, `getProviders`, `getProvider`, `getModels`, `getModel`,
  `stream`, `complete`, `streamSimple`, `completeSimple`).
- `./utils/transcript`: `normalizeContext`.
- `./api/bedrock-converse-stream`: `stream`, `streamSimple`.
- `./api/bedrock-converse-stream.lazy`: `bedrockConverseStreamApi`, `setBedrockProviderModule` (a facade module
  object keeps its Rust implementation; any other `{ stream, streamSimple }` is called through JS).
- `./bedrock-provider`: `bedrockProviderModule`.
- `./types`: types only.
- `./api/anthropic-messages`: `stream` (including `options.client`, a pre-built SDK client called through JS),
  `streamSimple`.
- `./api/anthropic-messages.lazy`: `anthropicMessagesApi`.
- `./api/pi-messages`: `stream`, `streamSimple`, `PiMessagesResponseError`.
- `./api/pi-messages.lazy`: `piMessagesApi`.
- `./api/transform-messages`: `splitErrorToolResultImages`, `transformMessages`.
- `./api/github-copilot-headers`: `inferCopilotInitiator`, `hasCopilotVisionInput`, `buildCopilotDynamicHeaders`.
- `./api/simple-options`: `clampMaxTokensToContext`, `resolveSamplingParams`, `buildBaseOptions`, `MIN_ANSWER_TOKENS`,
  `DEFAULT_THINKING_BUDGETS`, `clampReasoning`, `thinkingBudgetForLevel`, `clampThinkingBudgetToAnswerRoom`,
  `adjustMaxTokensForThinking`.
- `./api/google-shared`: every runtime export (`resolveGoogleThinkingLevel`, `convertMessages`, `convertTools`,
  `mapStopReason`, `retryGoogleRequest`, ...).
- `./api/google-generative-ai`, `./api/google-vertex`, `./api/mistral-conversations`: `stream`, `streamSimple`; their
  `.lazy` modules: `googleGenerativeAIApi`, `googleVertexApi`, `mistralConversationsApi`.
- `./api/openai-responses`, `./api/azure-openai-responses`: `stream`, `streamSimple`; `./api/openai-codex-responses`:
  `stream`, `streamSimple`, `getOpenAICodexWebSocketDebugStats`, `resetOpenAICodexWebSocketDebugStats`,
  `closeOpenAICodexWebSocketSessions`; their `.lazy` modules: `openAIResponsesApi`, `azureOpenAIResponsesApi`,
  `openAICodexResponsesApi`.
- `./api/openai-responses-shared`: `getResponsesToolSupport`, `convertResponsesMessages`, `convertResponsesTools`,
  `processResponsesStream`.
- `./api/openai-prompt-cache`, `./api/azure-openai-config`, `./api/constrained-sampling`: every runtime export.

- `.` and `./models` (models slice): `createProvider` with every input (auth, model callbacks, `api`, `images`,
  `classifiers`), `createModels` with `credentials`, `modelsStore` and `authContext` and every `Models` method but
  `login`/`logout`, `calculateCost`, `clampThinkingLevel`, `getSupportedThinkingLevels`, `getModelType`,
  `isModelType`, `hasApi`, `modelsAreEqual`, `ModelsError`, `AI_TELEMETRY_SCHEMA`, `lazyStream`, `lazyApi`,
  `InMemoryModelsStore` and the faux provider (`fauxProvider`, `createFauxCore`, `fauxText`, `fauxThinking`,
  `fauxToolCall`, `fauxAssistantMessage`).
- `./compat`: also `getModels`, `getProviders`, the api registry (`registerApiProvider`, `getApiProvider(s)`,
  `unregisterApiProviders`, `registerBuiltInApiProviders`, `resetApiProviders`), `registerFauxProvider`,
  `findEnvKeys`, `getEnvApiKey` and the env names, the image models and registry (`getImageModel(s)`,
  `getImageProviders`, `generateImages`, `registerImagesApiProvider`, `getImagesApiProvider`,
  `registerBuiltInImagesApiProviders`, `generateImagesOpenAICodex`, `generateImagesOpenRouter`) and the legacy
  `stream<Api>`/`streamSimple<Api>` aliases.
- `./providers/*`: every built-in provider factory and its `*.models` catalogs, `./providers/all` (catalog reads and
  collections), `./providers/radius` and `radius-config`, `./providers/openrouter` (`personalizeOpenRouterCatalog`),
  `./providers/faux`, `./providers/cloudflare-stream`, `./providers/opencode-headers`.
- `./api/openrouter-images`, `./api/openai-codex-images`: `generateImages`; `./api/typesafe-system-one`,
  `./api/openai-decisions`, `./api/cloudflare-workers-ai-system-one`: `classify`; `./api/llama-cpp-classify`:
  `classify` and its helpers; their `.lazy` modules; `./api/cloudflare` and `./api/cloudflare-ai-binding`.

## Not bridged yet

These throw `<name> is not bridged to the Rust core yet`; they belong to the auth facade:

- `Models.login`, `Models.logout`
- `login` of a Rust provider's `auth.apiKey` and `auth.oauth`

`options.telemetry` is not passed to the core yet: a JS `TelemetrySpan` does not reach implementations.

The other modules and exports of `@at-inc/pi-ai` are not in this package yet. No runtime value comes from the TS
package. `getModel` reads the Rust catalog, which is built from the same catalog data.

## Differences from the TS

- An event's `partial` holds the text up to that event. In the TS, the live partial may already hold text from
  later deltas of the same network chunk when an async consumer reads it. Both converge at `text_end`,
  `thinking_end` and `done`. `toolCall.arguments` reads the core's current arguments, as the TS's live object does.
- `PiMessagesResponseError` is a TS class: the Rust stream reports a backend error response as an error message, so
  there is no Rust object behind it.
- `buildBaseOptions` computes the data fields in Rust; `signal`, `telemetry`, `fetch` and the callbacks are the
  caller's own objects, as in the TS.
- `transformMessages` returns new message objects, also for messages it leaves unchanged, and calls
  `normalizeToolCallId` with a copy of the source message.
- `processResponsesStream` fills the caller's `output` and pushes into the caller's stream, but the core reads the
  events as JSON copies: `onProviderStreamEvent` gets a copy of each event, the events pushed for one input event see
  the message as it is after that event, a callback option that throws (`onImageGenerationCall`,
  `resolveServiceTier`, ...) is ignored, and an early rejection does not call the iterator's `return()`.
- `appendGrammarToolInputJsonDelta` and `closeGrammarToolInputJson` copy `buffer.input` across the boundary on each
  call, so a caller that streams a long input through them pays time linear in the input per call.
- The Codex request body is not zstd-compressed on wasm (the build has no zstd); the backend accepts it uncompressed.
- The Anthropic Messages `User-Agent` on wasm is `pi/<version> (browser)`; the TS on Node reports its platform and
  runtime. The host does not expose them to the core yet.
- Bedrock: the command input `onPayload` sees and the items `onProviderStreamEvent` sees are JSON, with blobs
  (`image.source.bytes`, `redactedContent`) as base64 text instead of `Uint8Array`, and an exception item is the
  modeled exception. The client always sends through the host's fetch, so HTTP(S) proxies and
  `AWS_BEDROCK_FORCE_HTTP1` do not apply, and it resolves credentials and the region before it sends (see
  `rust/crates/pi-ai/src/api/bedrock_client.rs` for the SDK differences).
- Faux provider: `state` is a live view of the core's counters (`cancelledDeferred` is a new array on each read),
  queued messages are copied into the core, and `createFauxCore()`'s `stream` and `streamSimple` both go through
  the provider's `streamSimple`. See `rust/crates/pi-ai/src/providers/faux.rs` for the core's own differences (event
  partials, deferred options, option keys a factory sees).
- `personalizeOpenRouterCatalog` returns the caller's object for a model the key leaves unchanged and the caller's
  array when nothing changes; the TS returns new objects for every kept chat model.
- `options.fetch === globalThis.fetch` is the host fetch, so the Google adapters accept it as in the TS. Any other
  JS fetch is called through JS.
- A classifier result's `answers` keeps every question id as an own property, `__proto__` included.
- With an already aborted signal the image APIs make no request; the TS hands the signal to the SDK, which rejects.
  Both report the same aborted result.
- `createAiBindingFetch` is written in the facade: the Rust module only checks that the binding has `fetch` and returns
  it, which is what the facade does with the JS binding.
