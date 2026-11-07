# @at-inc/pi-agent-core-rs

`@at-inc/pi-agent-core` backed by the pi Rust core (`rust/crates/pi-agent`). It has the same exports map and module
layout, the types of `@at-inc/pi-agent-core` (a peer dependency; the declaration files re-export its own) and a
runtime in wasm (`@at-inc/pi-core`). Its pi-ai runtime values (`EventStream`) come from `@at-inc/pi-ai-rs`. Switch to
it with a package alias under another name than the peer: `"@earendil-works/pi-agent-core":
"npm:@at-inc/pi-agent-core-rs@<version>"`.

## Bridged

Every export of `.`:

- `agent.ts`: `Agent` over the Rust agent (`AgentHandle`).
- `agent-loop.ts`: `agentLoop`, `agentLoopContinue`, `runAgentLoop`, `runAgentLoopContinue`, `runToolCall`,
  `DEFAULT_AGENT_IDENTITY`.
- `proxy.ts`: `streamProxy`.
- `stream-fn.ts`: `setDefaultStreamFn`.
- `types.ts`: types only.

No runtime value comes from the TS package.

## How state and events cross

- `Agent` keeps `state` in JS with the caller's objects and applies each event to it, as the TS does, so reading the
  state never crosses. Before a run it sends what changed since the last run: the model, the thinking level, and the
  messages from the first one that differs. The tools, the stream function and the public properties go with the run.
- Events go to JS as changes (`rust/crates/pi-wasm/src/agent/events.rs`): `message_update` as the pi-ai stream
  deltas over one live partial, `turn_end` and `agent_end` as indices into the run's `message_end` messages.
- A message, model or tool a run hands back to JS (in events, the stream function's context, hook contexts, the loop's
  result) is the caller's object when the run read it from JS, so the identities the TS gives hold.

## Differences from the TS

- `Agent` compares messages and the model by identity when it syncs before a run. A message object or `state.model`
  edited in place between runs is not seen by the Rust agent; assign a new object instead.
- The caller's objects are found again by value. When a run holds two equal messages, a callback may receive one in
  place of the other. Messages the core creates (assistant messages, tool results) are new objects, except the
  streamed assistant message, which is the live partial of its `message_update` events, as in the TS.
- Hook contexts are snapshots (see `pi_agent::types`): only `beforeToolCall`'s edits of `context.args` reach the run.
- `streamProxy` reads the context as a `Context` and normalizes it, which leaves a transcript unchanged. The proxy
  stream's own differences are listed in `rust/crates/pi-agent/src/proxy.rs`.

## Tests

`npm run test:vitest --workspace=@at-inc/pi-agent-core-rs` runs all of `packages/agent/test` against the facade
(`vitest.config.ts` maps their `../src/<module>` imports to `test/facade/<module>.ts`; no file is skipped), and
`test/*.test.ts` for the facade's own cases.
