# Codex App Server Harness Primitives

Status: design, 2026-09-03. This document names the smallest components needed
to add a headless Codex app-server harness without creating a second Build
conversation system.

## Product boundary

The new harness gives OpenAI models the generic Build agent experience already
used by the headless Claude carrier: turns enter through `AgentSession`, reported
work becomes `ActivityReport`, deliberate agent messages and completion arrive
through Build MCP, and the existing conversation stores and renders both. Git
and the worktree remain the record of file changes.

The SPA change is limited to Settings/default-harness selection and the provider
names/catalog entries needed to select this harness. No conversation, activity,
terminal, agent-surface, task, issue, branch, plan, diff, or review UI changes
belong to this feature.

## Required primitives

### `AgentProvider::CodexAppServer`

- **Boundary:** Add one persisted provider identity with wire id
  `codex_app_server`. Keep `AgentProvider::Codex` and its wire id `codex`
  unchanged; every existing `codex` agent therefore remains a Codex TUI agent.
- **Interface:** The existing `AgentProvider::{ALL, wire_id, from_wire, label}`
  and `ModelChoice.provider` persistence paths.
- **Hides:** The carrier choice. Persisted plans, runs, agents, defaults, and
  thread lineage store a concrete provider, never a `headless` boolean.
- **Naming:** `codex_app_server` is labelled `Codex`; the existing `codex`
  provider becomes `Codex TUI`, following the existing `Claude Code` / `Claude
  Code TUI` distinction. Both Codex providers initially delegate to one static
  Codex model/effort catalog so that the catalog facts are not copied.

This is additive serde evolution. A missing provider keeps its current default,
an unknown provider is refused at selection, and no migration rewrites a stored
`codex` token.

### Provider-owned protocol construction

`open_session` remains the one terminal-versus-protocol construction point, but
it must not name `AdkSession`. Add this small provider hook:

```rust
pub trait Harness {
    // Existing launch and catalog methods remain.
    fn open_protocol_session(
        &self,
        request: ProtocolSessionRequest<'_>,
    ) -> Result<OpenedSession, HarnessError>;
}

pub struct ProtocolSessionRequest<'a> {
    pub spec: &'a HarnessSpec,
    pub root: PathBuf,
    pub resume_session_id: Option<&'a str>,
}

pub struct OpenedSession {
    pub session: Arc<dyn AgentSession>,
    pub output: SessionOutput,
}
```

- **Boundary:** `Carrier::Protocol` calls the selected `Harness` hook. The ADK
  harness constructs `AdkSession`; the new harness constructs
  `CodexAppServerSession`. TUI providers use the existing terminal arm.
- **Interface:** One operation that returns the existing session and its already
  subscribed output stream.
- **Hides:** Protocol type, startup handshake, subprocess pipes, and provider
  event vocabulary. No code above `harness/` matches a protocol provider.

The default hook refuses protocol construction. Contract tests require every
provider with `has_terminal() == false` to open an activity-reporting protocol
session, while `open_session` keeps rejecting a session with neither terminal
nor activity.

### `CodexAppServerHarness`

- **Boundary:** A new provider module owns the `codex app-server --stdio`
  process specification, Codex model/effort config, Build MCP config, and the
  construction hook above.
- **Interface:** The existing `Harness` interface plus
  `open_protocol_session`.
- **Hides:** Codex argument ordering and config keys. Callers provide
  `ModelChoice`, `SpawnOptions`, and `HarnessContext`; they never build app-server
  JSON or Codex config.

Use one app-server process per Build agent session. This preserves the current
warm-session lifetime, makes process death equal session death, avoids thread
multiplexing across owners, and keeps each MCP token scoped to one agent.

### `AppServerConnection`

- **Boundary:** Own the child stdin/stdout JSONL transport and JSON-RPC request
  correlation. Wire messages omit `"jsonrpc":"2.0"`, as Codex requires.
- **Interface:** `request(method, params, expected_response) -> RequestId`,
  `notify(method, params)`, and `respond(id, result_or_error)`. Each write is one
  serialized line under one writer lock.
- **Hides:** Monotonic request-id allocation, the bounded pending-request map,
  out-of-order response matching, serialization, line framing, and response
  delivery to the state machine.

A response resolves exactly one pending request by id. A duplicate or unknown
response id is a protocol failure, not a guessed response. A correlated JSON-RPC
error is returned to the operation that issued the request with method and id
context. Notifications never enter the request map.

Server-initiated requests cannot be left unanswered. The required-now
configuration requests no approvals; an unexpected approval, elicitation, or
unknown server request receives an explicit unsupported/denied response and is
recorded as a protocol error. It is never auto-approved and never waits for UI
that this feature does not provide.

### `CodexSessionState`

- **Boundary:** The only owner of initialization, thread, active-turn, pending
  interrupt, liveness, active model, and last reported error state.
- **Interface:** Pure transitions over lifecycle `ConnectionEvent` and
  `SessionCommand` values, returning `SessionEffect` values for JSON-RPC writes,
  session-fact updates, bounded operational reports, or close. Item
  notifications go to `CodexActivityTranslator` instead.
- **Hides:** Codex method names and ordering from `AgentSession` and `app.rs`.

The state machine is:

```text
Starting
  -> Initializing(initialize request id)
  -> OpeningThread(thread/start or thread/resume request id)
  -> Waiting(thread id)
  -> Working(thread id, turn id)
  -> Waiting(thread id)
  -> Ending
  -> Ended(exit code)
```

Initialization sends one `initialize` request with Build client metadata, waits
for its matching success response, sends `initialized`, then opens the thread.
No thread or turn request is sent before that sequence succeeds. Repeated or
out-of-order lifecycle responses fail the session. A turn accepted while the
handshake is in progress waits in the bounded session queue and starts as soon
as the thread reaches `Waiting`; this covers the existing spawn-then-deliver
path without making `send_turn` wait on Codex.

An exact persisted `resume_session_id` selects `thread/resume { threadId, cwd,
... }`; absence selects `thread/start { cwd, ... }`. The response's `thread.id`
becomes `AgentSession::session_id()` and is persisted through the existing agent
resume-id path. The app-server provider does not use the Codex TUI's
cwd/`resume --last` transcript guess: without an exact app-server thread id it
starts fresh rather than adopting an unrelated global rollout.

`thread/start` and `thread/resume` set the configured model/effort, `cwd`,
`approvalPolicy: "never"`, and danger-full-access sandbox policy. The response's
model becomes `AgentSession::active_model()`. A failure at initialize or thread
open closes the activity stream and exposes the JSON-RPC error as the epitaph.

### Turn controller

- **Boundary:** Implements the existing `AgentSession::{send_turn,
  can_interrupt, interrupt}` contract from `CodexSessionState`.
- **Interface:** `send_turn(Turn)` returns when the session accepts the command,
  never when the model finishes; `interrupt()` returns after writing or queuing
  the cancellation.
- **Hides:** The choice among Codex turn methods and the active thread/turn ids.

In `Waiting`, `send_turn` issues `turn/start` with one text input. In `Working`,
it issues `turn/steer` with `threadId`, the active `expectedTurnId`, and the same
text input. A `turn/started` notification or correlated `turn/start` response
records the turn id idempotently. `turn/completed` is the sole turn boundary and
clears it for statuses `completed`, `failed`, and `interrupted`.

The state retains a steering input until the matching response accepts it. An
`activeTurnNotSteerable` response moves that input to the next-turn queue instead
of dropping the user's words; any other steering error becomes a bounded
`TaskUpdate` and the session's reported error. An interrupt request similarly
stays pending until its response or the matching turn completion clears it.

`can_interrupt` is true only while one turn is active and no interrupt is
pending. `interrupt` sends `turn/interrupt { threadId, turnId }`; it never kills
the process. A turn submitted after an interrupt request is held in a bounded
session-owned queue and starts only after `turn/completed`, so it cannot steer a
turn the user just cancelled. `turn/completed` is never Build completion:
`done` remains the only lifecycle report.

### `CodexActivityTranslator`

- **Boundary:** Converts notifications for this session's thread into the
  existing `ActivityReport` vocabulary. It does not mutate a Build thread or
  call app RPCs; the existing activity pump remains the only writer into the
  conversation.
- **Interface:** `translate(notification) -> Vec<ActivityReport>` plus
  `close_turn(turn_id) -> Vec<ActivityReport>` for unanswered calls.
- **Hides:** Codex item variants, item-id pairing, delta suppression, status
  mapping, and Build-MCP suppression.

The required mapping is one place:

| Codex notification/item | Existing report |
| --- | --- |
| completed `reasoning` item | `Reasoning` from its final summary |
| completed `agentMessage` item | `Narration` from its final text |
| started command, file change, web search, dynamic tool, collaboration tool, or non-Build MCP item | `ToolUse { call_id: item.id }` |
| matching completed item | `ToolResult` with `Ok`/`Error` from status, exit code, or error |
| retrying or terminal `error` notification | bounded `TaskUpdate`; terminal errors also become the session epitaph |

Delta notifications update the quiet clock but allocate no transcript and do
not mint one conversation row per token. A completed item is authoritative and
emits once. An open tool item is removed on completion; items still open at
`turn/completed` emit `Unanswered`. MCP items whose server is `build` emit
nothing because `post_thread_message` and `done` already arrive through their
real Build MCP path.

### Build MCP configuration

- **Boundary:** The Codex provider owns one config builder shared by its TUI and
  app-server harnesses. It names the `build-bridge mcp --task <owner>` command,
  owner-scoped enabled tools, required startup, and the existing
  `BRIDGE_MCP_SOCKET` / `BRIDGE_MCP_TOKEN` environment.
- **Interface:** A provider-private value converted to Codex `--config`
  arguments for either process specification.
- **Hides:** Codex quoting and tool allow-list syntax. The MCP server and daemon
  continue to authenticate and route reports exactly as they do now.

No app-server event substitutes for MCP. In particular, an agent response and
`turn/completed` do not advance a Build phase; only a validated Build `done`
call does.

### Process, error, and quiet status

- **Boundary:** `CodexAppServerSession` owns the child, stdin close, stdout and
  stderr drainers, exit status, activity sender, and `CodexSessionState`.
- **Interface:** The existing `AgentSession` methods. `status` reads reported
  protocol state, `quiet_for` measures time since the last accepted protocol
  message, `exited_within` handles pipe-close/reap lag, `epitaph` returns the
  last terminal protocol error or bounded stderr line, and `end` closes stdin,
  kills if needed, and always reaps.
- **Hides:** Threads, pipes, locks, and shutdown ordering.

`Working` lasts from accepted turn start until `turn/completed`, even during a
long silent model call, so the existing idle sweep does not report false quiet.
EOF, malformed required lifecycle messages, oversized frames, and correlation
violations close the activity sender; the existing activity pump performs the
tab and session-lineage death rites.

### Bounds and forward compatibility

Name each limit once beside the connection: maximum JSONL frame bytes, maximum
stderr line bytes, maximum pending requests, maximum queued turns, maximum open
items, and the existing activity broadcast backlog. Read frames with a capped
buffer rather than `BufRead::lines`, which can allocate an unbounded line before
validation. Clip all protocol-derived activity
summaries before broadcasting; raw command output, patches, arguments, and full
JSON objects never enter `ActivityReport`.

Unknown notification methods and unknown item variants are ignored after
updating the quiet clock and a bounded diagnostic counter. Unknown fields on
known messages are accepted. Unknown response ids, malformed envelopes,
oversized frames, and missing fields required for the current lifecycle
transition fail fast. This permits additive Codex protocol releases without
hiding loss of session control.

## Settings-only SPA work

The bridge adds `codex_app_server` to `models.list.providers` and accepts it in
`settings.get` / `settings.set.default_harness`. The current `default_harness`
field remains the only stored setting. Legacy `claude_mode` and `codex_mode`
compatibility keys remain derived aliases and do not become a second carrier
authority.

The SPA may add the new id and labels to its fallback provider vocabulary,
render it in the existing Default agent selector, and update the existing
generic Codex choice to resolve to `codex_app_server` only when that is the
account default. This is catalog/provider wiring, not a new control or surface:
the existing Codex card keeps the same label and markup. The SPA must not add a
Codex mode control or alter any conversation, activity, terminal, surface, task,
issue, branch, plan, diff, or review UI.

## Required now versus deferred

| Required now | Deferred |
| --- | --- |
| Additive `codex_app_server` identity and exact persisted thread id | Reinterpreting or migrating existing `codex` TUI agents |
| Provider-owned protocol construction | Sharing one app-server process across Build agents |
| Initialize, exact start/resume, turn start/steer/interrupt | Codex thread archive, fork, rollback, review, realtime, and other app-server surfaces |
| Completed-item translation to existing `ActivityReport` | Codex-specific `AgentSurfaces` for subagents, shells, plans, or checklists |
| Static shared Codex model/effort catalog | Dynamic `model/list` catalog and provider capability discovery |
| Text-only `Turn` input | Images, local files, audio, mentions, skills, and Build attachment delivery |
| `approvalPolicy: never`, danger-full-access, explicit refusal of unexpected server requests | Approval, permission, elicitation, and user-input UI |
| Settings/default-harness naming and selection only | Any conversation, activity, terminal, task, issue, branch, plan, diff, or review UI change |

## Tests and live fixtures

Implementation follows TDD. Unit tests first pin provider serde compatibility,
provider-owned construction, JSON-RPC out-of-order correlation and errors, the
initialization/start/resume state machine, start-versus-steer, interrupt queueing,
item pairing, Build-MCP suppression, every bound, unknown additive messages,
process EOF, quiet status, and kill-plus-reap. Existing generic activity-pump,
idle-sweep, resume-id persistence, settings, catalog, and old-record tests must
pass unchanged except for additive expected-provider lists and labels.

Before implementing the translator, capture a real, non-secret protocol session
from the minimum supported Codex CLI into
`bridge/tests/fixtures/codex-app-server/<version>/`. Capture both request and
server lines for initialize, thread start, thread resume, narration, reasoning,
successful and failed commands, a file change, a non-Build MCP call, Build
`done`, steering, interruption, retry/error, turn completion, and clean process
shutdown. Store the CLI version and generated-schema hash beside the JSONL.

The capture tool must use a temporary repository and test prompts, redact home
paths, account/workspace ids, tokens, URLs, and machine-specific values, then
validate every redacted line against the generated schema. Tests replay the
checked-in fixture with no network or local Codex account. Hand-written fixtures
cover only failure shapes that cannot be induced safely, and are labelled as
synthetic.

## Unresolved choice

The minimum supported Codex CLI version is not yet selected. Fixture capture
must establish the first version whose stable schema has all required methods
and item fields; startup should fail with an actionable version error below that
floor rather than probe behavior by trial and error.
