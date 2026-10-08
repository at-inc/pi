# durable

A small local coding agent on `@earendil-works/pi-durable`. One process owns the model runtime, the durable
Harness, its SQLite storage, and the TUI. It reuses pi's model runtime, auth, settings, system prompt, keybindings,
theme, and interactive components; the agent itself is the durable Harness with its built-in `CodingTools`.

```bash
node --import ./packages/coding-agent/src/experimental/source-resolver.ts packages/coding-agent/src/experimental/durable/main.ts
node --import ./packages/coding-agent/src/experimental/source-resolver.ts packages/coding-agent/src/experimental/durable/main.ts --continue
```

A new session honors the default model and thinking level from `settings.json`. Without a configured model, it
prefers `openai-codex/gpt-6-sol` when authenticated, retaining the fork's micro default; otherwise it uses pi's
normal model selection. `--continue` preserves the newest session's model for the current directory. Sessions live under
`~/.pi/agent/experimental/durable-sessions/<cwd-hash>/<session>/session.sqlite`; a lock keeps a second process out
(a lock left by a crash goes stale after 10 seconds, and the next start waits for that). Use `/login` or pi itself
to configure providers; credentials are shared.

## Existing JSONL-v3 sessions

The stable `pi` CLI still reads and writes JSONL-v3. To keep using an existing session there, no conversion is needed:

```bash
pi --session /path/to/session.jsonl
```

To try the same history with durable, run an explicit import from the directory where the imported agent should work:

```bash
PI_REPO=/path/to/pi
node --import "$PI_REPO/packages/coding-agent/src/experimental/source-resolver.ts" "$PI_REPO/packages/coding-agent/src/experimental/durable/main.ts" --import /path/to/session.jsonl
node --import "$PI_REPO/packages/coding-agent/src/experimental/source-resolver.ts" "$PI_REPO/packages/coding-agent/src/experimental/durable/main.ts" --session <returned-session-id>
```

`--import` converts and exits without opening the TUI, contacting a model provider, or executing historical tool calls.
It never writes to the source file. The destination contains `session.sqlite`, an exact `source.jsonl` archive, and
`import.json` with the source SHA-256, entry-ID mapping, branch mapping, and conversion warnings. Stop any writer to the
source session before importing. The destination working directory is the command's current directory; the original
header, including its old working directory, remains in the archive.

The original active branch becomes `main`; other branches become durable conversation forks and are selectable with
`/agents`. Message and tool-call contents, compaction boundaries, context edits, model selection, and thinking state are
converted. Each imported branch's model context is checked against the v3 projection before publication. Extension data
is retained as data, not executed as an extension; inspect the reported warnings before continuing a session that used
custom tools or extensions.

Durable moves an initial system message ahead of any preceding user messages in model context. Raw entry order,
contributions, and the source archive stay unchanged; later system updates remain in place. Historical context is
available through `conversation.context(context, { at: entryId })`, including on imported conversation forks.

Conversion happens in a private staging directory. Only a completed, closed SQLite database is published as a session;
failed imports are removed and staging directories are ignored by `--continue`. Importing the same bytes into the same
working directory returns the existing imported session, including any subsequent durable conversation, without
overwriting it. Changed source contents produce a new snapshot. The stable and durable copies do not synchronize after
import. Use `--session` with the returned ID to select an older imported snapshot explicitly.

Malformed JSON, duplicate IDs, broken references, unsupported versions, and unsupported message formats fail the import
rather than silently discarding history. Pending assistant messages are rejected; finish or repair the source session
with the stable CLI first. Aborted, failed, and deferred assistant messages remain in history and usage but are excluded
from model context, with a warning. Missing tool results become synthetic errors in model context; orphan tool results
are excluded with a warning. Neither historical tools nor deferred requests are rerun.

## What it shows

- **Durability:** every streamed partial, tool output, queue, and turn is committed. Kill the process in the middle
  of a tool call and start it again with `--continue`: the interrupted call gets an interrupted result and the turn
  finishes. Nothing in the TUI handles recovery; it only renders the conversation view.
- **One view:** the TUI renders `Conversation.viewState()`, the structural mount of the transcript and the built-in
  documents (`pi.live`, `pi.inbox`, `pi.agent`, `pi.usage`). Streaming, tool progress, the queue, retries,
  compaction status, the model, and usage all come from it. The footer shows cumulative input/output/cache tokens,
  cache-hit rate, cost, and context-window usage; context turns yellow above 70% and red above 90%, with `(auto)`
  when automatic compaction is enabled.
- **Subagents:** the `subagent` tool runs a task in a child conversation owned by the call. `/agents` switches the
  view to any conversation, also while the subagent works, and the editor then talks to it: steer it while busy, or
  keep chatting after the call returned. Esc aborts the shown conversation; aborting the main turn aborts its
  subagents.
- **Task graph:** a live panel of `Harness.taskGraph()`, shown by default and toggled with `/tasks`: every live task, what it waits on, and the
  conversations it owns.

## Commands

- submit: prompt while idle, steer while busy
- follow-up key (`app.message.followUp`): queue a follow-up
- Esc: abort the shown conversation's work, including a manual compaction
- `/model` or the model key: select a model for the shown conversation
- `/login`: select a provider and configure OAuth or an API key
- thinking key (Shift+Tab): cycle thinking levels
- `/compact [instructions]`: summarize older context; reports "Nothing to compact" when the context fits in
  `compaction.keepRecentTokens`
- `/agents`: switch conversations
- `/tasks`: hide or show the task panel
- tools expand key (Ctrl+O): expand tool output and compaction summaries
- clear key (Ctrl+C) or Ctrl+D: exit at once, unlike pi's clear-first Ctrl+C; work in flight resumes with
  `--continue`

## Layout

| file | role |
| --- | --- |
| `main.ts` | arguments, open, run, close |
| `sessions.ts` | session directories and the lock |
| `import.ts` | source archive, duplicate-import detection, and atomic SQLite publication |
| `legacy-v3.ts` | validated v3 records, conversation forks, and context verification |
| `login.ts` | provider login options for the selector |
| `usage.ts` | cache-hit rate from the active transcript |
| `runtime.ts` | Harness, registry, settings, environments; the plain `DurableView` and `DurableController` |
| `prompt.ts` | pi's system prompt sections (tools, rules, docs, AGENTS.md, skills, cwd) as one extension |
| `subagent.ts` | the foreground subagent tool |
| `tui.ts` | rendering with pi's interactive components |

Compaction thresholds, retry policy, queue modes, and request timeouts come from pi's `settings.json` as loaded at
startup, read through Harness settings getters at each use. pi's HTTP dispatcher is configured as in pi; without it some provider streams end
early.

A turn that ends without an answer shows a notice; one recovered after a restart does not, since only submissions
made by this process are watched.

Not here: sessions list and resume picker, interactive fork creation and tree navigation, extensions, prompt templates, images.
