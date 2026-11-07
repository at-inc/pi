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

The other modules and exports of `@at-inc/pi-ai` are not in this package yet. No runtime value comes from the TS
package. `getModel` reads the Rust catalog, which is built from the same catalog data.

## Differences from the TS

- An event's `partial` holds the text up to that event. In the TS, the live partial may already hold text from
  later deltas of the same network chunk when an async consumer reads it. Both converge at `text_end`,
  `thinking_end` and `done`. `toolCall.arguments` reads the core's current arguments, as the TS's live object does.
