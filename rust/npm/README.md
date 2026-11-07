# rust/npm: the Rust core behind the npm API

These packages ship the pi Rust core (`rust/crates`) behind the npm API of the TS packages, as planned in
[`rust/BINDINGS.md`](../BINDINGS.md). The core is one wasm module built from `rust/crates/pi-wasm` with wasm-bindgen.

- `pi-core` (`@at-inc/pi-core`, internal): the built wasm, the Node host and the Workers host, and the JS glue every
  facade shares (event streams).
- `pi-ai` (`@at-inc/pi-ai-rs`): the facade of `@at-inc/pi-ai`. It has the same `exports` map and module layout. Its
  types come from `@at-inc/pi-ai` and its runtime from wasm. It covers the first slice:
  `createProvider`, `createModels`, `openAICompletionsApi`, `stream`/`complete` (see [pi-ai/README.md](pi-ai/README.md)).
- `pi-mcp` (`@at-inc/pi-mcp-rs`): the facade of `@earendil-works/pi-mcp`, with every runtime export of `.`, `./oauth`
  and `./testing` (see [pi-mcp/README.md](pi-mcp/README.md)).
- `pi-agent-core` (`@at-inc/pi-agent-core-rs`): the facade of `@at-inc/pi-agent-core`, every export bridged (see
  [pi-agent-core/README.md](pi-agent-core/README.md)).

`rust/npm` is its own npm workspace root with its own `package-lock.json`, outside the root workspaces. The root
lockfile does not change when a facade is added.

## Build

Prerequisites, from the repository root:

```sh
npm ci --ignore-scripts                      # the TS packages: types, and the tests the facades run
node scripts/update-model-catalog-pin.mjs --hydrate
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked   # must equal wasm-bindgen in rust/Cargo.lock
cd rust/npm && npm ci --ignore-scripts
```

Then, in `rust/npm`:

- `npm run build:wasm` runs `cargo build --release` of pi-wasm for wasm32, then `wasm-bindgen --target web --weak-refs`
  into `pi-core/wasm`. The script fails if the CLI version differs from the crate version.
- `npm run build` builds the wasm and then `dist/` of both packages. `dist/` takes its types from `@at-inc/pi-ai`'s
  `dist`, so build the TS packages first (`npm run build` at the root, or telemetry, schema and ai).

`--target web` gives ES module glue with an explicit `initSync({ module })`. `pi-core/src/core.ts` calls it with the
wasm bytes (Node reads `pi_wasm_bg.wasm` next to the package) or with a compiled `WebAssembly.Module` (Workers import
the `.wasm` file). The `workerd` export condition selects the Workers entry.

## Test

In `rust/npm`:

- `npm run check`: biome and `tsc` over both packages, their tests and the bench.
- `npm run test:vitest`: the TS package's own `openai-completions-*` and utils tests (`packages/ai/test`) against the
  facade.
  `pi-ai/vitest.config.ts` resolves the tests' `../src/<module>` imports to `pi-ai/test/facade/<module>.ts`, which
  re-exports the facade module. A test that imports a module the facade does not provide fails to resolve, so it
  goes into `skipped` in that config with its reason; a single test that cannot pass goes into `skippedTests`.
- `npm run test:workers --workspace=@at-inc/pi-ai-rs`: bundles a Worker with the `workerd` condition, runs it in
  Miniflare and streams one response from a local server through it.
- `npm run bench`: facade against the TS package on one local stream (2,000 text deltas, then one tool call with
  100 KB of arguments). On a 4-core cloud machine, both take about 14 ms per stream (median 13.9 ms TS, 13.9 ms Rust).
- `node --conditions=source bench/utils.ts` in `pi-ai`: boundary cost of the bridged utils; see below.

The TS tests mock the `openai` SDK (`vi.mock("openai")`) and assert on what the TS passed to it. The core speaks HTTP,
so `pi-ai/test/facade/openai-bridge.ts` replaces the host's `fetch` while the SDK is mocked. The bridge turns each
Chat Completions or Responses request back into the SDK call the TS would make, and turns the mock's chunks or
thrown error back into an SSE response.

## How a call crosses

`streamSimple(model, context, options)` from `@at-inc/pi-ai-rs/compat`:

1. `pi-ai/src/compat.ts` calls `streamFrom((sink) => wasm.aiStreamSimple(model, context, options, sink))`.
2. `pi-wasm/src/ai/compat.rs` reads `model` and `context` as JSON values, splits `options` into data and callbacks,
   and starts the Rust stream. It forwards the stream's events to `sink` from a `spawn_local` task.
3. The core's HTTP goes through `JsHost` to the host's `fetch`, and the response body streams back chunk by chunk.
4. `pi-core/src/events.ts` (`AssistantMessageEventSink`) applies each event's change to one JS partial and pushes
   TS events into the `AssistantMessageEventStream` the caller iterates.

## Boundary conventions

Each rule below has one example from the slice.

- **Plain data crosses as JSON values** (`pi-wasm/src/json.rs`, `serde-wasm-bindgen` with maps as plain objects and
  `None` as `null`). Example: `normalizeContext(context)` is `aiNormalizeContext`, which reads a `pi_ai::Context`
  and returns the normalized transcript as a plain object.
- **Options split into data and callbacks** (`options.rs`). Data fields are read into the Rust options struct.
  Functions become Rust callbacks. A value that the typed field cannot hold stays in `extra`, because the TS does not
  validate options. Example: `streamSimple(..., { toolChoice: "required" })` reaches the request body as
  `tool_choice: "required"`, as in the TS.
- **Callbacks are JS functions awaited from Rust** (`callback.rs`). The call is `await fn.call(this, ...args)`, and a
  throw or a rejection becomes a `PiError`. Example: `onPayload(payload, model)` gets the payload as a JS object. If
  it returns `undefined`, the core sends the object it passed, including in-place edits.
- **Errors keep `name`, `message`, `status` and data fields** in both directions (`error.rs`). Example: an
  `onPayload` that throws `Object.assign(new TypeError("bad payload"), { code: "E_PAYLOAD" })` ends the stream with
  `errorMessage: "bad payload"` and a diagnostic error `{ name: "TypeError", message: "bad payload", code: "E_PAYLOAD" }`.
- **Abort signals map both ways** (`abort.rs`). Example: `controller.abort(new Error("stop now"))` on the caller's
  `options.signal` ends the stream with `stopReason: "aborted"`. The JS `fetch` the core called sees its own
  `init.signal` abort with the same reason.
- **Objects with identity are handles** owned by a facade class. Example: `createProvider` returns a `RustProvider`
  that owns a `ProviderHandle`. `openAICompletionsApi()` returns a JS `ProviderStreams` that carries its
  `NativeStreams` handle (`internal/native-api.ts`), so `createProvider({ api: openAICompletionsApi() })` hands the
  Rust implementation back to Rust without a JS hop. A JS implementation in the same `api` map is called through JS.
- **Event streams send changes, not the partial** (`events.rs`, `pi-core/src/events.ts`). A text delta is an append.
  A block start carries the block with empty text. A block end carries the whole block. A tool call delta marks the
  arguments stale, and the sink reads them from the `AssistantMessageHandle` only when JS reads
  `toolCall.arguments`. Example: for 2,000 text deltas, 2,000 appends cross and one block at `text_end`. Every
  event's `partial` is the same JS object.
- **Types stay TS.** Each facade module starts with `export type * from "@at-inc/pi-ai/<module>"` and types its values
  as `typeof ts.<name>`. The TS event stream classes have `#private` fields, which makes TypeScript compare them by
  declaration, so `pi-core` exports its copies of those classes typed as the TS classes.

## Writing a facade module

Follow these steps for each TS module (`packages/<pkg>/src/<path>.ts`):

1. Create `rust/npm/<pkg>/src/<path>.ts` with the TS file's name. It re-exports the TS module's types and defines
   each runtime value as `export const name: typeof ts.name = ...` over a wasm export.
2. Add the wasm exports in `rust/crates/pi-wasm/src/<pkg>/<module>.rs`, one file per facade module. Name each export
   with the package prefix plus the TS name (`aiOpenAICompletionsStream`). Use the helpers: `arg`/`throw`
   (options.rs) for arguments and results, `provider_stream_options`/`simple_stream_options` for options, and
   `EventSink` for streams.
3. Throw through `notBridged("<name>")` (`internal/not-bridged.ts`) for an input or a method that the Rust side
   does not bridge yet. Each later slice removes its own call sites.
4. When a module re-exports names that a TS star export also has, export the values by name (see `index.ts` and
   `compat.ts`). Two star exports of the same name are a TS2308 error.
5. Add `test/facade/<path>.ts` (re-export the module, and import `openai-bridge.ts` if its tests mock the SDK). Add
   the TS test files to `include` in `vitest.config.ts`. Then fix the facade or the Rust until they pass, or put
   each test file you cannot run in `skipped` with its reason.
6. Run `npm run check`, `npm run test:vitest` and the Rust gates (`cargo fmt`, `clippy` for the host and wasm32, and
   `cargo test`).

Notes for the follow-up slices:

- **pi-ai models and streaming**: add each API's `*.lazy.ts` (`aiAnthropicMessagesApi` and so on, each a
  `NativeStreams`) and its `api/*.ts` module. Then bridge the `notBridged` methods of `models.ts`. The test bridge
  already maps `/responses` to `client.responses.create`.
- **pi-ai auth and OAuth**: `createModels({ credentials, modelsStore, authContext })` and `Models.login`/`logout`.
  OAuth callback servers of pi-ai and pi-mcp listen through `pi_host::server::listen_http`: `PiHost.listen` over
  `node:http` on Node, and on Workers when `nodejs_compat` provides `node:http`, as in the TS.
- **pi-agent-core**: a new package `rust/npm/pi-agent-core` (`@at-inc/pi-agent-core-rs`) with exports prefixed
  `agent` and an `Agent` handle class. Tool `execute` and hooks are callbacks. JS event streams passed into Rust use
  `events::from_js`.

Each new package goes into `workspaces` in `rust/npm/package.json` and gets a `test:vitest` script. Add a CI step
for it to the `wasm` job in `.github/workflows/rust.yml`.

## Boundary cost of the utils

A util that takes messages, tools or a model reads them as JSON text (`JSON.stringify`, then `serde_json`), and
pays for the size of what crosses, where the TS function may read only a few fields. `pi-ai/bench/utils.ts` on a
200-message context (113 KB of JSON), median of 300 calls, on a 4-core cloud machine:

| Function | TS | Rust | Slower |
| --- | --- | --- | --- |
| `estimateContextTokens(context)` | 0.0028 ms | 0.449 ms | 160x |
| `normalizeContext(context)` | 0.0025 ms | 0.0137 ms | 5x |
| `getCurrentTools(messages)` | 0.0014 ms | 0.0916 ms | 66x |
| `estimateMessageTokens(message)` | 0.0004 ms | 0.0060 ms | 17x |
| `contentText(content)` | 0.0002 ms | 0.0026 ms | 15x |
| `isContextOverflow(message)` | 0.0001 ms | 0.0016 ms | 18x |
| `parseStreamingJson` (1 KB) | 0.0226 ms | 0.0277 ms | 1x |
| `validateToolArguments` | 0.0054 ms | 0.0142 ms | 3x |

`estimateContextTokens` crosses every message while the TS reads only the messages after the last usage.
`normalizeContext` and the other transcript helpers read only the system messages in Rust and keep the caller's other
message objects. The calls stay well under a millisecond, but code that calls them per event or per message should
cache the result. The core's own request path does not cross: it runs these in Rust on Rust values.
