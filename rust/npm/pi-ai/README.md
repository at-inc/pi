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

## Not bridged yet

These throw `<name> is not bridged to the Rust core yet`:

- `createProvider({ images, classifiers })`
- `createProvider({ fetchModels, filterModels, filterAllModels })`
- `createModels({ credentials, modelsStore, authContext })`
- `Models.setProvider` of a provider not made by `createProvider`
- These `Models` methods: `getModelsOfType`, `getModelOfType`, `getAllModels`, `refresh`, `checkAuth`, `getAvailable`,
  `getAvailableOfType`, `getAllAvailable`, `getAuth`, `login`, `logout`, `streamDeferred`, `fetchDeferred`,
  `cancelDeferred`, `generateImages`, `classify`

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
- The Anthropic Messages `User-Agent` on wasm is `pi/<version> (browser)`; the TS on Node reports its platform and
  runtime. The host does not expose them to the core yet.
- Bedrock: the command input `onPayload` sees and the items `onProviderStreamEvent` sees are JSON, with blobs
  (`image.source.bytes`, `redactedContent`) as base64 text instead of `Uint8Array`, and an exception item is the
  modeled exception. The client always sends through the host's fetch, so HTTP(S) proxies and
  `AWS_BEDROCK_FORCE_HTTP1` do not apply, and it resolves credentials and the region before it sends (see
  `rust/crates/pi-ai/src/api/bedrock_client.rs` for the SDK differences).
