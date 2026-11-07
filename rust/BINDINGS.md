# Bindings: the Rust core behind the npm API

This is the plan for the phase after the port: ship the Rust crates behind the existing npm API of `@at-inc/pi-ai`,
`@at-inc/pi-agent-core` and `@earendil-works/pi-mcp`, so bro-components can switch by changing its package aliases.

## Decision: one wasm build for Node and Workers

The core is single-threaded by design: it uses `Rc`/`RefCell` like the TS uses shared objects, every I/O goes through
`pi_host::Host`, and JS callbacks are everywhere (tool `execute`, `onPayload`, custom `fetch`, auth resolvers, agent
hooks). It already builds for `wasm32-unknown-unknown` (CI checks it).

- **wasm-bindgen** runs the core's `!Send` futures on the JS event loop (`wasm_bindgen_futures::spawn_local`), the same
  model as the TS. A JS callback is a direct call that returns a Promise.
- **napi-rs is not used.** Native code would need the core on its own thread, with every callback marshalled to the JS
  thread and back, and one native artifact per platform (including Windows). It would gain nothing the daemon needs.
- One artifact serves the Node daemon, the CLI and Cloudflare Workers (cloud-agent, api). Only the host differs.

## Layout

- `rust/crates/pi-wasm`: `cdylib` with the wasm-bindgen exports and `JsHost`, an implementation of `pi_host::Host` over
  JS functions passed in at init.
- `rust/npm/pi-ai`, `rust/npm/pi-agent-core`, `rust/npm/pi-mcp`: the TS facade packages, published as
  `@at-inc/pi-ai-rs`, `@at-inc/pi-agent-core-rs` and `@at-inc/pi-mcp-rs`. Each has the same `exports` map and module
  layout as the TS package it replaces, so bro-components switches with
  `'@earendil-works/pi-ai': 'npm:@at-inc/pi-ai-rs@x'`, the way it aliases today.
- The wasm module and the JS host glue live in a shared internal package, `rust/npm/pi-core`. Its entry instantiates
  the wasm with the host of the runtime (the `workerd` export condition selects the Workers entry) and exports the
  initialized `wasm` and the `host`. `@at-inc/pi-core/core` exports `initCore(host, module)` for other runtimes.
  The Node host has global `fetch`/`WebSocket`, timers, `process.env` and `node:fs` reads. The Workers host has
  `fetch`, `WebSocket`, timers, and `process.env` when `nodejs_compat` provides it, with no file system and no
  processes, like the TS on Workers. `pi_host::Host` has `spawn_process` (MCP stdio) and `listen_http` (OAuth
  callback servers): `cross-spawn` and `node:http` on Node, unsupported on Workers.
- Build: `wasm-bindgen --target web --weak-refs`. `--target web` gives ES module glue with an explicit `initSync`,
  which takes the wasm bytes (Node) or a compiled `WebAssembly.Module` (Workers `.wasm` imports). The CLI is pinned
  to the `wasm-bindgen` crate version in `Cargo.lock`.
- `rust/npm` is its own npm workspace root with its own lockfile, outside the root workspaces. See
  [`rust/npm/README.md`](npm/README.md) for build, tests and how to write a facade module.

## Boundary rules

- **Plain data crosses as JSON values** through `serde-wasm-bindgen`, with the Rust serde shapes, which match the TS
  JSON.
- **Objects with identity are handles.** Examples: `Models`, `Agent`, an event stream, `McpClient`, an
  `AbortController`. A facade TS class owns the wasm handle and has the TS class's methods.
- **JS callbacks are passed as functions.** The Rust side wraps them as `js_sys::Function` and awaits the returned
  Promise; a thrown error becomes a `PiError` with the JS name and message, and back.
- **Options split into data and callbacks.** The data fields are read into the Rust options struct. A value that the
  typed field cannot hold stays in `extra`, because the TS does not validate options and some adapters forward the
  raw value (`toolChoice: "required"` in openai-completions).
- **Event streams do not re-send the partial message.** Rust events carry a live shared `partial`. Sending it whole
  per event would cost O(message) per delta. Instead, an event crosses as its own fields plus the change to the
  partial: text, thinking and argument appends, a block start, a usage update. The facade keeps one JS
  `AssistantMessage` per stream and applies the change, so every event's `partial` is the same live object, as in the
  TS. A block start carries the block with empty text, and the deltas after it append. The core parses a whole
  network chunk before JS takes its first event, so the Rust partial may already hold later deltas. Sending the
  live block at the start would make every delta of that chunk a whole-block resend. Tool call arguments are read
  from the core only when JS reads `toolCall.arguments`.
- **Types stay TS.** Facade modules re-export the TS package's types; only runtime values come from wasm. Constant
  data that the TS exports as values (model tables, `Type` schema builders) may stay TS when the Rust side has
  nothing to add. Each case is listed in the facade's README. The TS event stream classes have `#private` fields,
  which makes TypeScript compare them by declaration. So `pi-core` ships copies of those classes typed as the TS
  classes.

## Verification

- The facade runs the TS packages' own vitest suites. A vitest config aliases the tests' `../src/...` imports to the
  facade modules. A test that cannot run against the facade is listed with its reason. Tests that mock the `openai`
  SDK run through a test bridge, which turns the core's HTTP requests back into calls on the mocked SDK.
- A Miniflare smoke test runs the facade on workerd.
- bro-components' type check and tests pass with the aliases switched, on Node (daemon) and on workerd (cloud-agent).

## Order

1. Foundation (done): `pi-wasm`, `JsHost`, `pi-core` with both hosts, the boundary helpers (JSON values, handles,
   callbacks, errors, abort signals, streams), the build, and one vertical slice end to end:
   `createProvider` + `openAICompletionsApi` + `stream`/`complete`, tested with the TS openai-completions tests.
2. Facades in parallel: pi-ai streaming and models; pi-ai auth and OAuth; pi-agent-core; pi-mcp.
3. bro-components: switch the aliases on a branch, then run its type check and tests and a daemon smoke run.
