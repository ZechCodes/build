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

Agent construction has one callable entry point and one provider dispatch:

```rust
pub trait Harness {
    // Existing launch and catalog methods remain.
    fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<OpenedSession, HarnessError>;
}

pub struct SessionOpenRequest {
    pub spec: HarnessSpec,
    pub root: PathBuf,
    pub choice: ModelChoice,
    pub terminal: TerminalOpenOptions,
    pub resume_session_id: Option<String>,
}

pub struct OpenedSession {
    pub session: Arc<dyn AgentSession>,
    pub output: SessionOutput,
}

pub fn open_session(
    provider: AgentProvider,
    request: SessionOpenRequest,
) -> Result<OpenedSession, HarnessError> {
    harness_for(provider).open_session(request)
}
```

- **Boundary:** Every agent spawn calls the free `open_session`; that function's
  call to `harness_for` is the only provider dispatch. `Harness::open_session`
  has one shared PTY default used by both TUI providers. The ADK and app-server
  harnesses override it to construct their own protocol sessions. A human shell
  calls the lower-level PTY constructor directly because it has no provider.
- **Interface:** One operation that returns the existing session and its already
  subscribed output stream.
- **Hides:** Protocol type, startup handshake, subprocess pipes, and provider
  event vocabulary. `Tab::spawn`, `Carrier`, and `app.rs` never match a provider
  or name `AdkSession` / `CodexAppServerSession`.

`TerminalOpenOptions` contains size, readiness grace, and optional transcript
locator. A protocol override consumes none of those terminal mechanics; it uses
the same request's `root`, model choice, process spec, and exact resume id.
Contract tests call the public construction function for every provider and
require `has_terminal() == false` to return an activity-reporting session. The
existing refusal of a session with neither terminal nor activity remains after
construction.

### `CodexAppServerHarness`

- **Boundary:** A new provider module owns the `codex app-server --stdio`
  process specification, conversion of `ModelChoice` into the 0.153.0 protocol
  fields, Build MCP config, and the construction override above.
- **Interface:** The existing `Harness` interface plus its callable
  `open_session` operation.
- **Hides:** Codex argument ordering, config keys, and model/effort JSON fields.
  Callers provide `ModelChoice`, `SpawnOptions`, and `HarnessContext`; they never
  build app-server JSON or Codex config.

Use one app-server process per Build agent session. This preserves the current
warm-session lifetime, makes process death equal session death, avoids thread
multiplexing across owners, and keeps each MCP token scoped to one agent.

### `AppServerConnection`

- **Boundary:** Own only stdin/stdout JSONL framing and JSON-RPC request
  correlation. It does not own the child process, session state, policy, or
  activity translation. Wire messages omit `"jsonrpc":"2.0"`, as Codex
  requires.
- **Interface:** `request(PendingOperation) -> Result<RequestId,
  ConnectionError>`, `notify(ClientNotification) -> Result<(),
  ConnectionError>`, `respond(ServerResponse) -> Result<(), ConnectionError>`,
  and `close() -> Result<(), ConnectionError>`. No other component writes
  app-server stdin.
- **Hides:** Monotonic checked request-id allocation, serialization, the bounded
  `RequestId -> PendingOperation` map, and out-of-order response matching.

`PendingOperation` is a typed enum with `Initialize`, `StartThread`,
`ResumeThread`, `StartTurn { input }`, `SteerTurn { turn_id, input }`, and
`InterruptTurn { turn_id }`. It owns each method and params shape, so callers
cannot pair a method with the wrong response expectation. The reader removes one
entry and emits `CorrelatedResponse { operation, result }`; `CodexSessionState`
never sees a raw response id and cannot maintain a second correlation map.

A duplicate/unknown response id, id exhaustion, a response carrying both result
and error or neither, and a response whose typed body does not match its
`PendingOperation` fail the connection. Notifications never enter the pending
map. Every write serializes first, enforces the outbound limit, writes one line
under one writer lock, flushes, and returns its exact I/O or encoding error.

### `ServerRequestPolicy`

- **Boundary:** Purely maps a typed server request and coordinator-supplied time to
  `ServerRequestDecision { response, after_response }`. The connection writes
  `response` first; only after that `Result` succeeds may the session apply
  `Continue`, `FailTurn(reason)`, or `FailSession(reason)`.
- **Interface:** `decide(ServerRequest, current_unix_seconds: i64) ->
  ServerRequestDecision`.
- **Hides:** Every known app-server callback and the required-now no-UI policy.

The session coordinator samples whole Unix seconds and passes that value to
`decide`; `ServerRequestPolicy` never reads a clock.

Request decoding classifies the method before validating method-specific
params. A known method enters only its focused typed params decoder. Any other
method becomes `ServerRequest::Unknown { id, method }` without inspecting
`params`; absent params and arbitrary valid JSON params, including scalars and
arrays, therefore receive JSON-RPC `-32601` method-not-found and then the
`FailSession` policy below. Malformed JSON-RPC envelopes still fail before
policy.

The exhaustive 0.153.0 policy is:

| Server request | Response | Decision after successful write |
| --- | --- | --- |
| `item/commandExecution/requestApproval`, `item/fileChange/requestApproval` | typed `decline` | Continue; report one bounded `TaskUpdate` |
| legacy `execCommandApproval`, `applyPatchApproval` | typed denied decision | Continue; report one bounded `TaskUpdate` |
| `mcpServer/elicitation/request` | `{ action: "decline" }` | Continue |
| `item/tool/requestUserInput` | JSON-RPC `-32601` unsupported | FailTurn: no answers may be invented |
| `item/permissions/requestApproval` | JSON-RPC `-32601` unsupported | FailTurn: no permission may be invented |
| `item/tool/call` | JSON-RPC `-32601` unsupported | FailTurn: dynamic tools are deferred |
| `account/chatgptAuthTokens/refresh` | JSON-RPC `-32601` unsupported | FailSession with an actionable re-authentication epitaph |
| `attestation/generate` | JSON-RPC `-32601` unsupported | FailSession; Build did not advertise attestation support |
| `currentTime/read` | `{ "currentTimeAt": <i64> }` | Continue |
| unknown method | JSON-RPC `-32601` method-not-found | FailSession after replying |

The launch requests `approvalPolicy: "never"`, so approval callbacks are
unexpected but still answered. Nothing is auto-approved and no request waits for
UI that this feature does not provide.

`Continue` leaves the turn and session open. `FailTurn` records the reason and,
when a turn is active, issues the ordinary typed `InterruptTurn`; the session
remains reusable after matching completion. `FailSession` records the epitaph,
closes activity, and runs the normal idempotent shutdown. If the response write
fails, connection failure wins and none of these follow-up actions runs.

### `AppServerProcess`

- **Boundary:** Own the `Child`, cached exit status, and bounded stderr drainer.
  It hands stdin/stdout to `AppServerConnection` exactly once and never parses
  protocol messages or allocates request ids.
- **Interface:** `spawn(spec, root, process_events) ->
  Result<(AppServerProcess, ConnectionPipes), HarnessError>`, `exit_code()`,
  `exited_within(timeout)`, `stderr_epitaph()`, and `shutdown() ->
  Result<(), HarnessError>`.
- **Hides:** Spawn setup, pipe extraction, signal-derived exit codes, wait/reap
  races, and stderr retention.

The process monitor and stderr drainer push typed
`ProcessEvent::{Exited(code), MonitorFailed(reason),
StderrDrainerFailed(reason)}` values asynchronously to the session coordinator.
They never wait for `AgentSession::status()` to discover failure and never
mutate `CodexSessionState` themselves. The coordinator owns the resulting
failure transition and shutdown.

`CodexAppServerSession::end` first calls idempotent `connection.close()` to send
EOF, then idempotent `process.shutdown()`. Shutdown returns the cached result if
already reaped; otherwise it calls `try_wait`, kills only a still-running child,
and always waits after kill. Concurrent `end`, EOF, and process-monitor
completion share the one cached result, so the child is killed at most once and
reaped exactly once.
Drop is only a backup that invokes the same shutdown path and cannot invent a
second process owner.

### `CodexSessionState`

- **Boundary:** The only owner of initialization, thread, active-turn, pending
  turn reconciliation, ordered user input, pending interrupt, liveness, active
  model, and last reported error state.
- **Interface:** Pure transitions over lifecycle `ConnectionEvent` and
  `SessionCommand` values, returning `SessionEffect` values for JSON-RPC writes,
  session-fact updates, bounded operational reports, or close. Item
  notifications go to `CodexActivityTranslator` instead.
- **Hides:** Codex method names and ordering from `AgentSession` and `app.rs`.

The state machine is:

```text
Starting
  -> Initializing
  -> OpeningThread(thread/start or thread/resume)
  -> Waiting(thread id)
  -> StartingTurn(thread id, observed turn id?, early completion?)
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

Initialization omits `experimentalApi` (equivalent to false), does not advertise
attestation or MCP elicitation extensions, and opts out of no notifications the
translator requires. Dynamic tools and other experimental methods therefore
cannot enter through an accidental capability opt-in.

`StartingTurn` maps to public `AgentStatus::Working`: the session has accepted
the user's turn even while the start response is being reconciled. `Working`
lasts through any pending steer/completion reconciliation that can still affect
delivery; it never falls back to the quiet clock.

An exact persisted `resume_session_id` selects `thread/resume { threadId, cwd,
... }`; absence selects `thread/start { cwd, ... }`. The response's `thread.id`
becomes `AgentSession::session_id()` and is persisted through the existing agent
resume-id path. The app-server provider does not use the Codex TUI's
cwd/`resume --last` transcript guess: without an exact app-server thread id it
starts fresh rather than adopting an unrelated global rollout.

For the pinned 0.153.0 schema, `thread/start` and `thread/resume` send optional
`model`, `cwd`, `approvalPolicy: "never"`, and `sandbox: "danger-full-access"`.
Neither method accepts effort. Every `turn/start` sends
the selected optional `model` and `effort`; those are the 0.153.0 sticky turn
fields. `turn/steer` accepts neither. `result.model` and
`result.reasoningEffort` from thread start/resume initialize the session facts;
the selected turn values update them when a turn is accepted.

`thread/started` may precede the start/resume response. In that case
`OpeningThread` records its candidate id but does not become ready. The
correlated response must name the same id, after which `Waiting` begins. A
response-first path becomes `Waiting` immediately and a later matching
notification is idempotent. A differing id or an error response after a
successful notification is a protocol contradiction and fails the session.

A failure at initialize or thread open closes the activity stream and exposes
the correlated JSON-RPC error as the epitaph.

### `ParentThreadFilter`

- **Boundary:** Routes every thread-scoped notification before typed lifecycle
  or activity handling.
- **Interface:** `classify(method, params, expected_parent_thread) ->
  ParentThreadRoute::{Parent, Child, Unscoped}`.
- **Hides:** The routing-field differences between `thread/started` and the
  other notification shapes.

The filter reads only the method's routing fields. During thread opening, a
`thread/started` value with `parentThreadId` is a child and a root value may
establish the candidate parent id; after opening, a thread-scoped notification
is parent-owned only when its `threadId` matches the exact active parent.
`Child` returns before full params decoding and before any lifecycle, quiet
clock, activity, error, epitaph, diagnostic, open-item, completed-item, or limit
mutation. A malformed child error therefore cannot contaminate or terminate the
parent session. A `subAgentActivity` item emitted on the parent thread remains
parent activity even though its payload describes a child agent.

### Turn controller

- **Boundary:** Implements the existing `AgentSession::{send_turn,
  can_interrupt, interrupt}` contract from `CodexSessionState`.
- **Interface:** `send_turn(Turn)` returns when the session accepts the command,
  never when the model finishes; `interrupt()` returns after writing or queuing
  the cancellation.
- **Hides:** The choice among Codex turn methods and the active thread/turn ids.

In `Waiting`, `send_turn` retains the input in `PendingOperation::StartTurn`,
writes `turn/start`, and enters `StartingTurn`. In `Working`, it retains the
input in `PendingOperation::SteerTurn` and writes `turn/steer` with `threadId`
and the exact active `expectedTurnId`. At most one steer request is in flight;
later inputs enter the ordered bounded queue and are not written until that
steer is reconciled. This serializes steers and preserves user order.

`StartingTurn` resolves every legal ordering:

| Observed order | Reconciliation |
| --- | --- |
| response, then `turn/started` | Response id enters `Working`; the same notification is idempotent |
| `turn/started`, then response | Record the observed id but remain `StartingTurn`; response must match before entering `Working` |
| `turn/completed`, then response | Record id/status, close open items, remain `StartingTurn`; matching response settles directly to `Waiting` and never resurrects `Working` |
| `turn/started`, completion, response | Both notifications are retained; matching response settles directly to `Waiting` |
| response error after a start/completion notification | Fail: the same operation cannot both start and fail |

Any turn-scoped notification received in `StartingTurn` may establish the
observed turn id before `turn/started`; all later response and notification ids
must match it. This lets early item events reach the translator without treating
them as another turn.

Any notification-before-response or completion-before-response reconciliation
must receive its correlated response within the five-second reconciliation
limit. Timeout fails the session; it never guesses whether the request was
accepted.

An exact duplicate start/completion notification is a no-op. A second id for the
same start, a completion for another active id, a steer success naming another
id, or any other turn-id disagreement fails the session. `Waiting` retains the
last completed id only to ignore an exact duplicate completion; any other
completion while waiting is desynchronization.

A steer input remains owned by its `PendingOperation` until one of these
outcomes:

- Success must return the expected active turn id. The input was accepted once
  and is never replayed.
- `SteerErrorKind::NoActiveTurn` is only JSON-RPC `-32600` with the canonical
  no-active-turn message. It is the normal completion race, not
  `activeTurnNotSteerable`. If matching `turn/completed` was already
  seen, replay the retained input as the next `turn/start`. If the response wins
  the race, hold it provisionally until matching completion arrives, then
  replay. Absence of that completion for five seconds is a protocol failure.
- `SteerErrorKind::ActiveTurnNotSteerable` is the structured
  `codexErrorInfo.activeTurnNotSteerable` variant. It means the current
  review/compact turn still exists but cannot accept same-turn input. Keep the
  input queued until that exact turn completes, then send it as the next
  `turn/start`; if completion was already retained, start it immediately.
- Any other error reports the retained input as undelivered and fails the
  session. Build never silently drops or guesses delivery.

If `turn/completed` arrives while a steer response is pending, the state closes
the turn but does not send another queued input until the response establishes
whether the retained steer was accepted or must be replayed. Thus
completion-before-response cannot duplicate or lose a message.

`can_interrupt` is true only while one turn is active and no interrupt is
pending. `interrupt` sends `turn/interrupt { threadId, turnId }`; it never kills
the process. A turn submitted after an interrupt request is held in a bounded
session-owned queue and starts only after `turn/completed`, so it cannot steer a
turn the user just cancelled. A completion before the interrupt response is
retained against the pending operation; later success or `-32600` no-active-turn
is satisfied, while another error is reported without reopening the completed
turn. `turn/completed` is never Build completion: `done` remains the only
lifecycle report.

### `CodexActivityTranslator`

- **Boundary:** Converts notifications for this session's thread into the
  existing `ActivityReport` vocabulary. It does not mutate a Build thread or
  call app RPCs; the existing activity pump remains the only writer into the
  conversation.
- **Interface:** `translate(notification) -> Vec<ActivityReport>` plus
  `close_turn(turn_id) -> Vec<ActivityReport>` for unanswered calls.
- **Hides:** Codex item variants, item-id pairing, delta suppression, status
  mapping, completed-item deduplication, and suppression.

`classify_item(item) -> ItemClassification` is the only item-type switch.
`ItemClassification` is a typed enum with `TrackedTool { summary:
ToolSummaryCategory }`, `Emitting { report: ItemReportKind }`, and `Suppressed {
reason: SuppressionReason }`. It owns both lifecycle behavior and the tool
summary category, so start/completion handlers do not repeat item lists or
reclassify an item.

The exhaustive classification and report mapping is one place:

| Codex item | Classification and existing report |
| --- | --- |
| `reasoning` | `Emitting { report: Reasoning }`; completion emits `Reasoning` from its final summary |
| `agentMessage` | `Emitting { report: Narration }`; completion emits `Narration` from its final text |
| `commandExecution`, `fileChange`, `webSearch`, `imageView`, `sleep`, `imageGeneration`, `collabAgentToolCall`, non-Build `mcpToolCall` | `TrackedTool` with its corresponding `ToolSummaryCategory`; start emits `ToolUse { call_id: item.id }`, and matching completion emits `ToolResult` with `Ok`/`Error` from status, exit code, or error |
| `subAgentActivity` | `Emitting { report: SubAgentActivity }`; emits bounded `TaskUpdate` from `agentPath` and `kind` (`started`, `interacted`, `interrupted`, or `completed`) |
| `contextCompaction` | `Emitting { report: ContextCompaction }`; start/completion emit bounded `TaskUpdate` |
| Build `mcpToolCall` | `Suppressed { reason: BuildMcp }` |
| `dynamicToolCall` | `Suppressed { reason: DeferredDynamicTool }` |
| unknown item | `Suppressed { reason: UnknownItem }` |

Delta notifications update the quiet clock but allocate no transcript and do
not mint one conversation row per token. Error notifications are outside item
classification: each emits a bounded `TaskUpdate`, and a terminal parent error
also becomes the session epitaph. An open tracked tool is removed on completion;
tracked tools still open at `turn/completed` emit `Unanswered`.

`CompletedItemLedger` is a provider-owned, per-session LRU of `(turn_id,
item_id)` keys. Production retains at most 256 keys and 128 KiB of aggregate
retained turn/item-id UTF-8 bytes. A duplicate key is suppressed and refreshed
to newest. Before inserting a new key, the ledger evicts oldest keys until both
limits admit it. A key larger than the byte limit is processed but not retained;
inability to retain a completion is never fatal. All keys for a turn are removed
by `close_turn(turn_id)`.

Classification happens before requiring an item id or touching either ledger.
Suppressed Build-MCP, deferred dynamic-tool, and unknown items therefore emit
nothing and consume no open-item or completed-item count/byte capacity. For
tracked and emitting items, a key still in `CompletedItemLedger` suppresses both
duplicate completion and a late duplicate start. The ledger never terminates a
session solely because many valid items complete during a long turn. Its finite
memory deliberately means a duplicate replayed after eviction, or a duplicate
of an individually unretainable key, may emit again; suppression is guaranteed
only while the key remains in the window.

Build MCP remains suppressed because `post_thread_message` and `done` already
arrive through their real Build MCP path.

Dynamic tools are deferred: the harness does not advertise them, execute
`item/tool/call`, or translate externally introduced dynamic-tool items. Their
typed suppressed classification prevents accidental partial implementation of
that surface.

The harness does not set `features.multi_agent` or otherwise force native
delegation. Existing Codex configuration remains authoritative. When Codex
naturally emits `collabAgentToolCall` or `subAgentActivity`, the mappings above
keep that work visible in the generic conversation. Codex-specific subagent
surfaces remain deferred.

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

- **Boundary:** `CodexAppServerSession` composes, but does not merge,
  `AppServerProcess`, `AppServerConnection`, `CodexSessionState`,
  `ServerRequestPolicy`, the translator, and the activity sender.
- **Interface:** The existing `AgentSession` methods. `status` reads the
  coordinator-published state, `quiet_for` measures time since the last accepted
  parent protocol message, `exited_within` delegates process/reap lag, `epitaph`
  reads the coordinator-published terminal reason, and `end` runs the idempotent
  close-then-shutdown sequence.
- **Hides:** The coordination loop. Process ownership stays in
  `AppServerProcess`; request IDs stay in `AppServerConnection`; domain state
  stays in `CodexSessionState`.

`Working` lasts from accepted turn start until `turn/completed`, even during a
long silent model call, so the existing idle sweep does not report false quiet.
EOF, malformed required lifecycle messages, oversized frames, and correlation
violations close the activity sender; the existing activity pump performs the
tab and session-lineage death rites.

`AgentSession::status()` is a pure snapshot read. It does not poll the child,
inspect drainer health, transition state, close activity, or initiate shutdown.
Process and drainer failures arrive through `ProcessEvent` and are handled by
the coordinator even if no caller polls status.

The protocol reader sends decoded events in wire order and then exactly one
`ReaderFinished` event after it reaches EOF or its terminal decode error. Any
terminal trigger may begin idempotent process shutdown immediately, but the
coordinator does not publish `Ended`, close the activity sender, or publish the
final epitaph until `ReaderFinished` has been processed. Final epitaph priority
is a terminal protocol/session error delivered by the reader, then a typed
process or drainer failure, then the retained stderr tail. Buffered stdout
errors therefore outrank stderr fallback deterministically even when process
exit is observed first.

### Bounds and forward compatibility

`AppServerLimits` is the one value passed to process, connection, state, and
translator. Production uses these conservative defaults:

| Limit | Value |
| --- | ---: |
| inbound JSONL frame | 1 MiB |
| outbound JSONL frame | 1 MiB |
| one retained stderr line | 16 KiB |
| all retained stderr text | 32 KiB |
| correlated pending requests | 64 |
| queued user inputs | 16 |
| aggregate queued input UTF-8 bytes | 256 KiB |
| open translated items | 256 |
| aggregate open-item ids/summaries | 128 KiB |
| completed-item deduplication keys | 256 |
| aggregate completed-item key bytes | 128 KiB |
| one emitted activity summary | existing 240-character summary limit |
| notification/response reconciliation | 5 seconds |
| activity broadcast backlog | existing 1,024 reports |

The stdout decoder incrementally reads into a buffer capped at 1 MiB plus one
sentinel byte. Newline at or below the cap yields exactly one JSON value. On the
first excess byte, the decoder enters an oversized-frame discard state, retains
no more payload, and consumes through that frame's newline using only its fixed
buffer and fixed read scratch space. It then returns the one frame-too-large
error and closes the connection; no unread suffix of the oversized frame can be
parsed as a second frame. EOF while discarding returns the same terminal
oversized-frame error. Invalid UTF-8/JSON, a blank frame, or trailing
non-whitespace after the value also fails the connection. The writer serializes
into a capped buffer before taking the lock.

Queue insertion checks both count and aggregate UTF-8 bytes before mutation.
Open-item insertion checks both item count and aggregate retained bytes; item
completion removes its charge. Overflow refuses the new operation and fails the
session with the named limit. Completed-item insertion instead evicts least
recently used keys to satisfy both ledger limits and never fails the session for
ledger capacity. Stderr is always drained to prevent child deadlock. Its rolling
decoder retains at most 16 KiB for one line, discards that line's excess bytes
until newline, and retains at most the newest 32 KiB across lines.
Protocol-derived summaries are clipped before broadcast; raw command output,
patches, arguments, image data, and full JSON objects never enter
`ActivityReport`.

Parent-owned unknown notification methods and unknown item variants are ignored
after updating the quiet clock and a bounded diagnostic counter. Unknown fields
on known messages are accepted. Unknown response ids, malformed envelopes,
oversized frames, and missing fields required for the current lifecycle
transition fail fast. This permits additive Codex protocol releases without
hiding loss of session control.

Static method lookup tables and focused request, notification, lifecycle, and
item handlers keep every dispatch function under the `~10-path` complexity
target. Item behavior and summary selection delegate to the one typed
`ItemClassification` rather than duplicating type matches in multiple handlers.

### Protocol version floor

The minimum supported version is `codex-cli 0.153.0`. This design is pinned to
the stable files emitted by:

```text
codex app-server generate-json-schema --out <dir>
```

on 0.153.0, specifically `v1/InitializeResponse.json`,
`v2/ThreadStartParams.json`, `v2/ThreadResumeParams.json`,
`v2/TurnStartParams.json`, `ClientRequest.json`, `ServerRequest.json`, and
`ServerNotification.json`. A live 0.153.0 initialize probe on 2026-09-03 returned
`userAgent: "build_probe/0.153.0 (...)"`; the generated initialize schema also
requires `userAgent`.

`CodexSessionState` validates the semantic version in
`initialize.result.userAgent` before sending `initialized`. It parses only the
first whitespace-delimited component and requires that entire component to be
`<clientInfo.name>/<Codex semver>`, using the name sent in the initialize
request. It never searches later user-agent text for a version. If that leading
component does not parse, the pure state returns `RequireVersionEvidence`; the
session coordinator asks `CodexAppServerHarness`'s `CodexVersionProbe` and feeds
the typed result back as a state event. The probe runs `codex --version`, expects
`codex-cli <semver>`, and caches the result by resolved binary path for the
daemon lifetime. The cache is shared by all sessions; there is never a probe per
turn. A version below 0.153.0 from either source, or failure to obtain parseable
evidence from both, fails startup with the required and observed values.

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
| Natural collaboration activity translation under existing Codex config | Forcing native delegation or pinning `features.multi_agent` |
| Typed refusal of `item/tool/call` | Dynamic tool registration, execution, and translation |
| Settings/default-harness naming and selection only | Any conversation, activity, terminal, task, issue, branch, plan, diff, or review UI change |

## Tests and live fixtures

Implementation follows TDD. Each matrix row starts as a failing test:

| Area | Required race and failure cases |
| --- | --- |
| Identity/construction | old `codex` records reopen on TUI; `codex_app_server` round-trips; every provider is opened through free `open_session`; no caller above `harness_for` dispatches on provider; no-terminal sessions report activity |
| Correlation | responses resolve out of order to the right `PendingOperation`; duplicate/unknown id, id exhaustion, wrong typed body, result-plus-error, and result-less response fail; pending-map overflow is unchanged state |
| Writes | request, notification, server response, flush, and close each propagate encoding/size/I/O failure; failed request write removes its pending entry |
| Initialize/version | no request precedes initialize; queued first turn waits; initialize error fails; leading matching-name 0.153.0 user-agent passes; wrong-name, 0.152.x, unparsable, missing, and later-text-only versions use/fail through the one cached probe as specified; `initialized` is sent once |
| Thread open | fresh uses start, exact id uses resume, no exact id never guesses; notification-before-response and response-before-notification converge; matching duplicates are inert; id mismatch and error-after-notification fail |
| Model/effort | thread start/resume send `model` but no effort; turn start sends `model` plus `effort`; steer sends neither; response model/effort update session facts |
| Starting turn | response-start, start-response, completion-response, start-completion-response, and response-completion converge; completion-before-response never resurrects Working; error after observed start/completion fails; duplicate completion is inert and wrong turn id fails |
| Steer serialization | one steer is written while later inputs queue; success releases the next input in order; returned turn-id mismatch fails; queue count/byte overflow fails without partial insertion |
| Steer completion race | completion then `-32600` and `-32600` then completion each replay retained input exactly once as a new turn; success after completion never replays; missing completion reaches the five-second failure |
| Non-steerable turn | `activeTurnNotSteerable` does not pretend completion; retained input waits behind the same turn and starts once after its matching completion |
| Interrupt | duplicate interrupt is a no-op; completion before response plus later success or `-32600` stays completed; queued post-interrupt input starts only after completion; interrupt never kills the process |
| Server requests | one table-driven case for every `ServerRequest` variant asserts exact response bytes and after-response decision; injected time produces `{ "currentTimeAt": <i64> }` without a clock read; response-write failure prevents the decision; unknown methods with absent, object, array, scalar, or null params reply `-32601` before session failure, while malformed known-method params fail typed decoding; no path approves |
| Parent isolation | child lifecycle, item, delta, malformed error, terminal error, and unknown notifications cause no state, quiet-clock, activity, error, epitaph, diagnostic, ledger, or limit mutation; root `thread/started` still establishes the candidate parent; parent-thread `subAgentActivity` remains visible |
| Translation | the one typed classification covers every item variant and tool summary category; each required item emits the stated report once; tool result pairs by item id; natural collaboration events remain visible; suppressed Build MCP, dynamic, and unknown items require no id and consume no ledger capacity; open calls close `Unanswered` at turn end |
| Completed-item ledger | duplicates suppress completion and late start while retained and refresh recency; count and byte pressure evict oldest keys without failure; individually oversized keys process without retention; long turns exceeding the window remain live; evicted duplicates document the finite-window tradeoff; turn close clears only that turn's keys |
| Bounds/decoder | exact-limit frames pass; limit-plus-one is discarded through newline at fixed capacity before one terminal error; an oversized suffix is never decoded as another frame; no-newline, invalid UTF-8, invalid/trailing JSON, and blank frames fail at bounded allocation; aggregate queue/open-item limits release bytes on removal; stderr drains while retained bytes stay capped |
| Process/liveness | stdout EOF closes activity only after `ReaderFinished`; process exit, monitor failure, and drainer failure reach the coordinator without status polling; `status()` is a pure read; buffered protocol error wins over process/drainer failure and stderr fallback regardless of arrival race; close/end/drop kill at most once and reap exactly once; signal exits have stable codes |
| Dispatch shape | method lookup selects focused typed request and notification handlers; the single item classifier selects lifecycle behavior and summary category; each dispatch function remains under the `~10-path` complexity target |
| Compatibility/UI | existing activity pump, idle sweep, resume persistence, store fixtures, and MCP `done` tests pass; settings lists the new provider; only allowed provider/default wiring changes in the SPA |

Before implementing the translator, capture a real, non-secret protocol session
from the minimum supported Codex CLI into
`bridge/tests/fixtures/codex-app-server/<version>/`. Capture both request and
server lines for initialize, thread start, thread resume, narration, reasoning,
successful and failed commands, a file change, a non-Build MCP call, Build
`done`, steering, interruption, retry/error, turn completion, and clean process
shutdown. Include naturally emitted `collabAgentToolCall` and
`subAgentActivity` when available, but do not force delegation to obtain them.
Store the CLI version and generated-schema hash beside the JSONL.

The capture tool must use a temporary repository and test prompts, redact home
paths, account/workspace ids, tokens, URLs, and machine-specific values, then
validate every redacted line against the generated schema. Tests replay the
checked-in fixture with no network or local Codex account. Hand-written fixtures
cover only failure shapes that cannot be induced safely, and are labelled as
synthetic.
