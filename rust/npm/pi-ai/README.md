# @at-inc/pi-ai-rs

`@at-inc/pi-ai` backed by the pi Rust core. It has the same exports map and module layout, the types of
`@at-inc/pi-ai` (a peer dependency) and a runtime in wasm (`@at-inc/pi-core`). Switch to it with a package alias:
`"@at-inc/pi-ai": "npm:@at-inc/pi-ai-rs@<version>"`.

## Bridged

- `.`: `createProvider`, `createModels`, `EventStream`, `AssistantMessageEventStream`,
  `createAssistantMessageEventStream`, and the values the TS index re-exports from `utils/diagnostics`, `identity`,
  `json-parse`, `overflow`, `retry`, `streaming-json`, `string-enum`, `text` (`contentText`, `getSystemMessageText`,
  `renderSystemMessageUpdate`), `transcript`, `uuid` (`uuidv7`), `validation` and `version` (`PI_AI_VERSION`), and from
  `providers/faux`.
- `./compat`: the above, plus `getModel`, `stream`, `complete`, `streamSimple`, `completeSimple` (dispatch by
  `model.api` with env API keys) and `openAICompletionsApi`.
- `./api/openai-completions`: `stream`, `streamSimple`, `convertMessages`.
- `./api/openai-completions.lazy`: `openAICompletionsApi`.
- `./models`: `createProvider` (static providers with api-key auth, Rust or JS API implementations) and `createModels`
  (`setProvider`, `deleteProvider`, `clearProviders`, `getProviders`, `getProvider`, `getModels`, `getModel`,
  `stream`, `complete`, `streamSimple`, `completeSimple`).
- `./utils/*`: every export of `abort`, `abort-signals`, `diagnostics`, `error-body`, `estimate`, `event-stream`, `hash`,
  `headers`, `identity`, `json-parse`, `node-http-proxy`, `oauth-page`, `openrouter-cost`, `overflow`, `provider-env`,
  `provider-retry`, `retry`, `sanitize-unicode`, `sleep`, `streaming-json`, `string-enum`, `text`, `transcript`, `uuid`,
  `validation`, `version`, and `kimiCodingEndpoints` of `kimi-coding`. `abort`, `abort-signals` and `sleep` are JS, as
  in the TS: they work on the caller's `AbortSignal` objects and timers, and return them (`operationSignal(signal)` is
  `signal`), which a crossing into the core would replace. `withClientIdentity` is a JS wrapper of the caller's
  `Models` that merges identities in Rust.
- `./api/simple-options`: every export. `buildBaseOptions` copies the caller's option values (signals, callbacks,
  `fetch`) as they are and computes `samplingParams` and `maxTokens` in Rust.
- `./providers/faux`: `fauxText`, `fauxThinking`, `fauxToolCall`, `fauxAssistantMessage`.
- `./types`: types only.

## Not bridged yet

These throw `<name> is not bridged to the Rust core yet`:

- `createProvider({ images, classifiers })`
- `createProvider({ fetchModels, filterModels, filterAllModels })`
- `createModels({ credentials, modelsStore, authContext })`
- `Models.setProvider` of a provider not made by `createProvider`
- These `Models` methods: `getModelsOfType`, `getModelOfType`, `getAllModels`, `refresh`, `checkAuth`, `getAvailable`,
  `getAvailableOfType`, `getAllAvailable`, `getAuth`, `login`, `logout`, `streamDeferred`, `fetchDeferred`,
  `cancelDeferred`, `generateImages`, `classify`
- `selectKimiCodingEndpoints` (`utils/kimi-coding`): it prompts through the auth interaction (pi-ai auth slice)
- `createFauxCore`, `fauxProvider` (`providers/faux`): the faux provider (pi-ai models slice)

The other modules and exports of `@at-inc/pi-ai` are not in this package yet. Of the root index these are the values
of `api/lazy` (`lazyApi`, `lazyStream`), `auth/*`, `models` (`ModelsError`, `AI_TELEMETRY_SCHEMA`, `getModelType`,
`isModelType`, `hasApi`, `calculateCost`, `getSupportedThinkingLevels`, `clampThinkingLevel`, `modelsAreEqual`),
`models-store` and `session-resources`, and the modules `utils/model-operations` and `utils/models-error`; they
belong to the models and auth slices. No runtime value comes from the TS
package. `getModel` reads the Rust catalog, which is built from the same catalog data.

## Differences from the TS

- An event's `partial` holds the text up to that event. In the TS, the live partial may already hold text from
  later deltas of the same network chunk when an async consumer reads it. Both converge at `text_end`,
  `thinking_end` and `done`. `toolCall.arguments` reads the core's current arguments, as the TS's live object does.
- Strings cross as Rust strings, which cannot hold an unpaired surrogate. One in an argument reads as U+FFFD (for
  example in `validateToolArguments`, `parseStreamingJson` and messages), except in `sanitizeSurrogates`, which works
  on the UTF-16 code units. `StreamingJsonParser` and `ToolCallArgumentsStream` join a surrogate pair split across
  pieces, but hold back a high surrogate at the end of the text read so far, where the TS shows it alone.
- `StreamingJsonParser.snapshot()` returns the same object while the value is unchanged, but each changed snapshot
  crosses whole, so it does not share completed containers with the previous one and a long stream costs time and
  memory quadratic in its length. `ToolCallArgumentsStream` shows every piece at once (no preview lag). The core's
  own providers parse arguments in Rust and do not pay this.
- Parsed JSON with a number that overflows (`1e400`) has `null` where the TS has `Infinity` (`parseStreamingJson`,
  `StreamingJsonParser`).
- Messages, tools and identities cross as JSON text (`JSON.stringify`), so functions and `undefined` fields are
  dropped and TypeBox's non-enumerable kind marks are not read; a result that holds them is a JSON copy. Transcript
  helpers read only system messages and return the caller's other message objects.

## `Type`

The TS index has not exported `Type` since 6536f801 (`Type`, `Static` and `TSchema` were removed, breaking), so the
facade exports none. Tool schemas from TypeBox or `@earendil-works/pi-schema`'s builders are plain JSON Schema at the
boundary; `validateToolArguments` compiles them with the Rust pi-schema.
