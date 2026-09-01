# Agent Surfaces Primitives

Status: design, 2026-09-01. The component list for `Agent Surfaces Spec.md`.
The spec is locked; this names the parts that implement it and draws the lines
between them.

## The rule this document exists to hold

One responsibility, one named component, one file. Four surface kinds share one
snapshot shape, one row renderer, one state-mark renderer, one open-state
memory and one action path — so adding a fifth kind is a parser and a renderer,
never a second copy of the machinery.

Read with `Agent Surfaces Spec.md` open. Where the two disagree, the spec wins
and this document is wrong — except for the one deviation named under
"Where this deviates from the spec", which is a spec bug reported to its owner.

---

## Bridge

### `AgentSurfaces`

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new)
- **Responsibility** The one snapshot value — `workflows`, `subagents`,
  `shells`, `checklist` — and its wire serialization, with an empty kind
  omitted rather than sent empty.
- **Reuses** `serde` with `skip_serializing_if` exactly as `bridge/src/thread.rs`
  writes `ThreadEvent`; `SurfaceAgent` is the single struct the spec's shared
  `agent` entry describes, used by both `workflows[].phases[].agents` and
  `subagents`.
- **Never** carries a transcript, a prompt body, a script, a file path or
  anything else unbounded; never knows a harness name; never grows a field the
  SPA cannot render.
- **The one serializer, and how `call_sequence` gets onto it**
  `AgentSurfaces::wire_value(call_sequence_of: &dyn Fn(&str) -> Option<u64>)
  -> serde_json::Value` is the ONLY thing that turns a snapshot into wire JSON.
  It serializes itself through serde and, as it writes each `subagents[]`
  entry, calls `call_sequence_of` with that entry's `spawning_call_id` and adds
  `call_sequence` when the closure answers. The app layer supplies the closure
  (see `agent_digest` surfaces field) and writes no JSON of its own, so there is
  never a second, hand-written serializer for the same objects in a second
  file. The harness layer still holds no sequence type: it holds a `&dyn Fn`
  for the length of one call.

### `SurfaceAgent::spawning_call_id`

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new)
- **Responsibility** Hold the harness `tool_use_id` of the Agent call that
  spawned a subagent, so the app layer can translate it into the spec's
  `call_sequence` without the ledger ever knowing what a thread sequence is.
- **Reuses** the `local_agent` `tool_use_id` the ledger already reads to key the
  subagent; `#[serde(skip)]`, so it is internal state on a wire struct and
  never reaches the SPA — the SPA is handed `call_sequence` and nothing else.
- **Never** appears on the wire; never carries a sequence (the harness layer has
  no thread); never set for a workflow agent, which has no spawning call the
  reader minted a row for.

### `SurfaceLedger`

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new)
- **Responsibility** The mutable state one session's stream builds
  `AgentSurfaces` in, and the only place a stream-json line — or a shell tail —
  is turned into surface state. It is also the ONLY judge of whether the
  snapshot moved.
- **Where the value lives** On `ProtocolState` (`bridge/src/harness/adk.rs:217`),
  as a `surfaces: SurfaceLedger` field. `ProtocolState` is already the
  `Arc<Mutex<_>>` the stdout reader writes and the session reads
  (`AdkSession.state`, `bridge/src/harness/adk.rs:321`), so the ledger has ONE
  owner, one lock, and no second shared cell to reason about, and the
  "called from under the app-wide lock, never blocks" rule is the same rule
  `AdkSession::status` (`bridge/src/harness/adk.rs:574`) already obeys against
  the same mutex. Two writers (the stdout reader and the shell tail poller) and
  one reader (`AgentSession::surfaces`) all reach it through that lock.
- **Reuses** `serde_json::Value` field reads in the manner of `adk.rs`'s
  `task_description`; the wholesale-replace discipline `read_task_roster`
  already uses for the background-task roster; `ShellTail` as a plain value
  handed in from `shell_tail.rs`.
- **Never** touches the filesystem, the broadcast channel or the app state;
  never merges a `workflow_progress` array (it is a full snapshot — replace);
  never claims a `state` token it does not recognise; never keeps a general
  call-id map (`ProtocolReader.calls` is the one of those).
- **Shape** Five entry points, one per line class, so `adk.rs` matches on
  `subtype` and this matches on `task_type`:
  - `read_task_event(subtype, &Value) -> bool`
  - `read_tool_call(name, &Value) -> bool`
  - `read_tool_answer(tool, call_id, &Value) -> bool` — the tool name comes
    from `RecordedCall::Minted { tool }` and the `&Value` is the WHOLE event,
    because `tool_use_result` (`TaskCreate`'s `{task:{id,subject}}`, Bash's
    `backgroundTaskId` and its output path) sits at the event level, not in the
    content block `ProtocolReader::read_tool_result` unwraps
    (`bridge/tests/fixtures/claude-stream/shell-and-checklist.jsonl:28`).
  - `read_shell_tail(shell_id, ShellTail) -> bool`
  - `snapshot() -> Option<AgentSurfaces>`

  Each of the four readers returns whether the snapshot moved, and that boolean
  is the ONLY trigger for a revision bump — from the stdout reader and from the
  poller alike, both bumping through the one `SurfaceRevision::bump()`, and only
  on `true`.

  Plus one internal accessor the poller needs and the wire never sees:
  - `running_shell_outputs() -> Vec<(String, PathBuf)>` — the shell id and the
    output path of every shell the ledger currently holds in `running`. It is
    the only way the poller learns which files to tail and the only way it
    learns there is nothing left to tail. `AgentSurfaces` carries no path, so
    this answer exists nowhere else.

  The one call-id map the ledger keeps is
  `pending_checklist_creates: HashMap<String, (String, String)>` — a `TaskCreate`
  call's subject and description, held only until its answer names the task id,
  and drained on answer. It is named for that job and holds nothing else.

### `SurfaceRevision`

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new)
- **Responsibility** The one way anything says "the snapshot moved". A newtype
  over `Arc<tokio::sync::watch::Sender<u64>>` with one method, `bump()`, which
  increments the counter the channel carries.
- **Why a named value rather than a method on the session** The stdout reader is
  a `std::thread` closure built from `ProtocolReader` BEFORE `AdkSession` is
  constructed (`bridge/src/harness/adk.rs:373-393`), and the tail poller is
  spawned the same way. Neither holds an `AdkSession`, so a
  `AdkSession::note_surfaces_moved()` would be unreachable from both of its
  callers. A cheap clonable value is: one is cloned into `ProtocolReader`, one
  into the poller, one kept on `AdkSession`.
- **Reuses** `tokio::sync::watch`, already the shape `spawn_activity_pump`
  selects on; the `Arc` clone-into-the-reader-thread pattern
  `AdkSession::spawn` already uses for `state` and `activity`.
- **Never** carries the snapshot (the signal is content-free — the reader asks
  for the snapshot); never bumps on a reader that returned `false`; never
  blocks a sender (a watch send to no receiver is not an error).

### the four kind parsers

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new), private to `SurfaceLedger`
- **Responsibility** One function per kind, each owning the fields the spec's
  "Where the data comes from" section pins for it:
  `apply_workflow` (`local_workflow` + `workflow_progress`),
  `apply_subagent` (`local_agent`, which is also where `spawning_call_id` is
  set from the started event's `tool_use_id`),
  `apply_shell` (`local_bash` + the launch path recorded off the Bash result +
  the tail handed in by `read_shell_tail`),
  `apply_checklist` (`TaskCreate` / `TaskUpdate` / `TodoWrite`).
- **Reuses** `SurfaceAgent`, so workflow agents and subagents are one shape
  built by one constructor; the two state tables below, so no parser maps a
  harness token to a wire token itself.
- **Never** duplicates another kind's field mapping; never mints an activity
  row (rows are the reader's business, snapshots are this one's); never parses
  a field as required — every field on this wire is optional and pinned to a
  fixture.
- **The two state tables, so four parsers do not each own a copy** Private
  functions in `surfaces.rs`, used by all four parsers and by nothing else:
  - `wire_task_state(status: &str) -> Option<&'static str>` — for the
    `task_updated` / `task_notification` vocabulary (`completed`, `failed`,
    `killed`, `timed_out`, …). Terminal-and-failed → `"failed"`, terminal →
    `"done"`, anything else → `None`, which means LEAVE THE CURRENT STATE
    ALONE rather than write a state the harness did not claim.
  - `wire_agent_state(token: &str, has_started_at: bool) -> Option<&'static str>`
    — for the `workflow_progress` vocabulary (`start`, `progress`, `done`).
    `start` / `progress` → `"running"` when the entry carries `startedAt`,
    `"queued"` when it carries only `queuedAt`; `done` → `"done"`; an
    unrecognised token → `None`, and the `state` field is omitted entirely.
- **Where the terminal/failed vocabulary itself lives** `task_status_is_terminal`
  (`bridge/src/harness/adk.rs:1146`) becomes `pub(crate)` and gains one sibling
  beside it, `pub(crate) fn task_status_failed(status: &str) -> bool`, holding
  the `failed | error | timed_out` set that is inline in `ended_summary`
  (`bridge/src/harness/adk.rs:1163`) today. `ended_summary` is rewritten over
  `task_status_failed`, so the set exists once and both files read the same
  one. If a future kind needs it outside `adk.rs` and `surfaces.rs`, the pair
  moves whole into `bridge/src/harness/task_status.rs` — never copied.
- **The workflow-agent id rule, stated here because the spec cannot be met
  verbatim** The spec makes `agent.id` required; a queued workflow agent has no
  `agentId` (`bridge/tests/fixtures/claude-stream/workflow.jsonl:40`, agent
  index 2, `char-counter`: `state: "start"`, `queuedAt`, `model: "haiku"`, no
  `agentId`). `apply_workflow` synthesises
  `id = agentId.unwrap_or(format!("{workflow_task_id}:{index}"))`. The `index`
  is fixed for the workflow's life, so the id is stable until the real
  `agentId` arrives and the row takes a new identity ONCE, on the tick the
  agent starts. Dropping the entry instead would make the pill count and the
  phase done-count lie, which is worse than one identity change. This is the
  only place an id is invented; `surfaceRows`'s `${kind}-${index}` fallback is
  a paint-safety net for malformed input, not a substitute for it.
- **The one shell-state rule, stated here so it has one home** `apply_shell` is
  the only writer of a shell's `state` and `exit_code`. `task_notification` is
  authoritative; the `[exited with code N]` marker `ShellTail` parses is
  recorded ONLY while the shell has no exit code from a notification yet. A
  notification arriving after the marker overwrites it; a marker arriving after
  a notification changes nothing.

### `ShellTail`

- **Layer** bridge
- **File** `bridge/src/harness/shell_tail.rs` (new), with its own tests for the
  bounded read, the marker parse, and an untrusted path
- **Responsibility** Read the last 20 lines of one shell's output file and the
  `[exited with code N]` marker at its end, as one pure value.
- **Reuses** nothing; it is a bounded file read with a cap named once.
- **Never** reads a path the harness did not name in its own Bash result;
  never reads the whole file into memory; never runs while no shell is
  running; never decides what a shell's state is — it reports the marker and
  `apply_shell` decides.
- **Why not in `surfaces.rs`** `surfaces.rs` is pure over `serde_json::Value`
  and strings and says so in the ledger's Never line. A filesystem reader in
  that file would contradict the file's own rule and make it the one file that
  grows for every new concern.

### `ShellTailPoller`

- **Layer** bridge
- **File** `bridge/src/harness/adk.rs`, owned by `ProtocolReader`
- **Responsibility** Tail every running shell once a second and hand each tail
  to `SurfaceLedger::read_shell_tail`, for as long as the ledger holds a
  running shell and no longer.
- **Who starts it** `ProtocolReader` gains one field,
  `shell_poller: Arc<Mutex<Option<JoinHandle<()>>>>`, and one private method,
  `ensure_shell_tail_poller()`. The reader calls it after any
  `read_task_event` or `read_tool_answer` that returned `true`, and only when
  `running_shell_outputs()` is non-empty and the slot is empty or holds a
  finished handle. That is the whole start rule: a `local_bash` starting after
  a previous poller exited on an empty set re-spawns one on the very event that
  made the set non-empty again, and nothing else can spawn one.
- **Who stops it** Itself. The loop sleeps one second, reads
  `running_shell_outputs()`, tails each path through `ShellTail`, calls
  `read_shell_tail` for each, bumps the `SurfaceRevision` once per `true`, and
  RETURNS when either `running_shell_outputs()` is empty or the `ActivitySlot`
  reads `None`. The `ActivitySlot` going `None` is how the stdout reader
  already announces its own end (`bridge/src/harness/adk.rs:373-393`), so
  "never outlives the stdout reader" needs no second flag and no second
  channel.
- **Reuses** `ShellTail`; `SurfaceRevision::bump()`, the same bump the stdout
  reader uses; `ActivitySlot` as the close signal; the `std::thread::spawn`
  reader pattern `AdkSession::spawn` already uses for stdout and stderr.
- **Never** runs on an empty shell set; never decides whether a tail moved the
  snapshot (the ledger's boolean decides, and the poller bumps only on `true`);
  never writes a shell's state or exit code; never learns a path from anywhere
  but `running_shell_outputs()`; never outlives the session's stdout reader.

### `AgentSession::surfaces` / `AgentSession::surfaces_changed`

- **Layer** bridge
- **File** `bridge/src/harness/session.rs`
- **Responsibility** The capability accessor — this session's snapshot, or
  `None` — and the watch channel that says the snapshot moved. On `AdkSession`,
  `surfaces()` is `self.state.lock().unwrap().surfaces.snapshot()` and
  `surfaces_changed()` is `self.revision.subscribe()`.
- **Reuses** the exact shape of `AgentSession::terminal` and
  `AgentSession::activity`: a defaulted method returning `None`, so a carrier
  that offers nothing writes not one line about surfaces; the same
  `state` mutex `status()` already takes.
- **Never** blocks (it is called from under the app-wide state lock, like
  `can_interrupt`); never sends content on the watch channel — the signal is
  content-free and the reader asks for the snapshot.

### `RecordedCall::Minted { tool }`

- **Layer** bridge
- **File** `bridge/src/harness/adk.rs`
- **Responsibility** Carry the tool's name on the call record
  `ProtocolReader.calls` already keeps, so an answer can be routed by tool
  without a second call-id map anywhere.
- **Reuses** the map itself (`bridge/src/harness/adk.rs:676`), inserted at
  `read_tool_call` and taken at `read_tool_result` exactly as today —
  `BuildsOwn` is unchanged and still returns early.
- **Never** becomes a second pairing record; never survives the answer (the
  entry is taken, not read, which is what keeps a long session's map bounded).
- **Consequence** `read_tool_result` calls
  `ledger.read_tool_answer(&tool, &call_id, &event)` with the whole event, and
  the ledger no longer needs to guess a `TaskCreate` answer from a Bash answer.

### `ActivityReport`

- **Layer** bridge
- **File** `bridge/src/harness/session.rs`
- **Responsibility** One reported activity plus the tool call that spawned the
  agent reporting it (`parent_call_id`), which is what the activity broadcast
  now carries.
- **Reuses** `AgentActivity` unchanged — the five kinds stay five kinds, and the
  parent link rides beside them rather than being repeated as a field on four
  variants.
- **Never** appears in the conversation vocabulary: `ThreadEventKind` gains no
  variant for it, because a subagent's reasoning is reasoning.
- **Consequence** `AdkSession::read_message` stops dropping messages carrying
  `parent_tool_use_id` and reports them with the parent set instead.

### `ThreadEventDraft` and `ThreadEvent.parent_sequence`

- **Layer** bridge
- **File** `bridge/src/thread.rs`
- **Responsibility** One value describing an event about to be minted (kind,
  summary, session id, revision id, links, parent sequence), and the one field
  that folds a row under the tool call that spawned it.
- **Reuses** `Thread::push_event` and `Thread::push_event_with_links`, which
  become thin constructors over the draft and keep their signatures — so no
  existing call site moves and the next field is added in one place.
- **Never** makes `parent_sequence` required; never mints a parented row when
  the parent sequence is unknown (the row is minted flat instead, which is what
  the conversation already does for an answer whose call it could not pair).

### `Tab::call_sequences`

- **Layer** bridge
- **File** `bridge/src/app.rs` (the `Tab` struct, `bridge/src/app.rs:671`)
- **Responsibility** The session's one record of harness call id against the
  thread sequence of the row that call minted, AND whether that call has been
  answered. It is the single answer to all three of "which row does this child
  fold under" (`parent_sequence`), "which row spawned this subagent"
  (`call_sequence`) and "which calls died unanswered when the session closed".
- **Shape** `call_sequences: HashMap<String, MintedCallRow>`, where
  `MintedCallRow { sequence: u64, answered: bool }`. The `answered` flag is
  what makes a session-lived map safe: today the pump's local `open_calls`
  is a bare `HashMap<String, u64>` that is REMOVED from on answer
  (`bridge/src/app.rs:18576`) precisely so the `Closed` arm
  (`bridge/src/app.rs:18403`) can resolve everything left as `Unanswered`. A
  map that keeps answered entries and no flag would hand every answered call in
  the session back to `Thread::resolve_tool_call`
  (`bridge/src/thread.rs:1911`), which overwrites `outcome` and appends the
  answer text unconditionally — every finished call in the session would end
  up reading `unanswered — no answer, session ended`.
- **Reuses** the pairing the activity pump already computes — today as
  `open_calls`, a local inside the tokio task in `spawn_activity_pump`
  (`bridge/src/app.rs:18362`). Moving it onto `Tab` changes where it lives and
  adds the flag; it holds nothing else.
- **Never** a second map: `record_activity` reads and writes this one, and
  `agent_digest` reads this one. Nothing else keeps a call-id-to-sequence
  record in the app layer.
- **Lifetime** Kept for the session's life rather than dropped on answer —
  a subagent's surface entry outlives the Agent call's answer, so a map that
  forgot on answer could not stamp `call_sequence`. The pump's `Closed` arm
  resolves ONLY the rows with `answered == false`, then clears the map beside
  the `tab.live = false` it already writes there, so a dead session's map dies
  with it.
- **Pinned by** a test in which a session's call is answered `ok`, the session
  then closes, and the row still reads `outcome: ok` — while the subagent that
  call spawned still carries `call_sequence` on the detail payload after the
  answer landed.

### `record_activity` parent pairing

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** Turn an `ActivityReport`'s `parent_call_id` into the
  spawning row's `parent_sequence` on the row it mints, and record every call
  it mints a row for into the tab's `call_sequences`.
- **Reuses** `Tab::call_sequences` — the one map, read for the parent and
  written for the newly minted call. The pump no longer carries a local
  `open_calls` and passes no map.
- **The one change to the answer path** the `ToolResult` arm sets
  `answered = true` on the row instead of removing it, and reads the sequence
  off the row it just marked. Nothing else about the arm changes: an answer for
  a call the map never held still mints the flat `ToolResult` row it mints
  today.
- **Never** holds a parent link for a call the map has forgotten; never mints a
  parented row for one of Build's own MCP calls (those are silent by
  construction); never removes an entry (only the `Closed` arm clears).

### `DigestScope`

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** The two-variant enum — `List` and `Detail` — that says
  whether the digests a view is building may carry `surfaces`. It is a
  PARAMETER, passed explicitly by each handler, and it is never derived from
  anything.
- **Where it is passed** A second parameter on `agent_digests` / `agent_digest`
  and on `plan_view` / `run_view`:
  - `Detail` — `plan_get` (`bridge/src/app.rs:9628`, which `issue.get` aliases
    at `bridge/src/app.rs:5866`), `run_get` (`bridge/src/app.rs:10962`),
    `branch_get` (`bridge/src/app.rs:13214`), and every mutation answer that
    returns the same detail view (`answer_plan_mutation`,
    `answer_run_mutation`, `issue_view_full`, `open_implementation_run`
    (`bridge/src/app.rs:10959`), and the `run_view(.., thread_detail(params))`
    answers at `bridge/src/app.rs:11351`, `:11844`, `:11876`, `:11922`,
    `:11945`, `:12452`, `:12464`). See "Which payloads
    carry surfaces" in the Wire section for why the mutations are `Detail`.
  - `List` — `board_list` (`bridge/src/app.rs:12708`, `:12723`), `plan_list`
    (`bridge/src/app.rs:9647`), `issue_list` (`bridge/src/app.rs:9656`), the
    archived-plan list (`bridge/src/app.rs:13666`), the work-item candidates
    (`branch_candidate_from_run`, `bridge/src/app.rs:12866`; `issue_candidate`,
    `bridge/src/app.rs:13081`), `agent_list` (`bridge/src/app.rs:8320`),
    `agent_add` (`bridge/src/app.rs:8177`) and `agent_remove`
    (`bridge/src/app.rs:8275`).
- **Never derived from `ThreadDetail`** The obvious-looking rule
  "`ThreadDetail::Digest` means no surfaces" is WRONG and must not be written.
  `view_thread_detail` (`bridge/src/app.rs:15239`) hands `ThreadDetail::Digest`
  to `plan_view` / `run_view` whenever the caller named an `agent_id` or
  carried a thread cursor, because `detail_thread_value`
  (`bridge/src/app.rs:5511`) replaces the view's `thread` afterwards — and the
  agent rail's steady-state poll (`spa/src/core/agentRail.js:489-497`) always
  sends `agent_id`, a cursor, or both. Deriving would mean the one consumer
  that paints the pills is the one consumer that never receives them.
- **The branch.get seam** `branch_get` finds its row through `work_items`, whose
  candidates are built at `List` scope, and the rail reads a branch's agents off
  the ROW (`row.agents || (row.run && row.run.agents)`,
  `spa/src/core/agentRailModel.js:266`) rather than off the run view. So
  `branch_get` overwrites the found row's `agents` with the `agents` of the
  `Detail`-scope run view it just built. The two arrays are the same
  `agent_digests(run_id)` call today, so this changes no existing field — it is
  what puts the surfaces where the rail actually reads them, without a second
  session read per agent.
- **Pinned by** two tests: `branch.get` with `agent_id` AND
  `thread_after_sequence` carries `agents[].surfaces` when the session has any;
  `board.list` never carries the key at all.

### `agent_digest` surfaces field

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** Attach `surfaces` to an agent digest when the caller passed
  `DigestScope::Detail`, and supply the closure that stamps each subagent
  entry's `call_sequence`. The harness layer never learns what a sequence is;
  this is the one place the two halves meet.
- **How the two halves meet, exactly** `agent_digest` takes the session's
  snapshot once, then calls
  `snapshot.wire_value(&|call_id| tab.call_sequences.get(call_id).map(|row| row.sequence))`.
  `AgentSurfaces::wire_value` owns the JSON; `Tab::call_sequences` owns the
  sequences; `agent_digest` owns nothing but the closure that joins them, and
  writes no JSON for a `subagents[]` entry itself.
- **Reuses** `DigestScope` as the parameter, `Tab::call_sequences`,
  `AgentSurfaces::wire_value` and `agent_of_tab`
  (`bridge/src/app.rs:18515`).
- **Never** rides a list-shaped payload; never re-reads the session more than
  once per digest; never ships a kind with nothing in it; never inspects
  `ThreadDetail` to decide anything.
- **The list-shaped answers keep no surfaces** `agent.list`
  (`bridge/src/app.rs:8316`), `agent.add` (`bridge/src/app.rs:8145`) and
  `agent.remove` (`bridge/src/app.rs:8228`) pass `DigestScope::List`. The SPA
  has no caller of `agent.list` at all.

### surface invalidation in `spawn_activity_pump`

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** Select on the session's surface-revision channel beside the
  activity stream and note the owning entity changed, so progress lines that
  mint no row still stale the detail.
- **Reuses** `AppState::note_entity_changed` → `ChangeBus::note_entity`
  (`bridge/src/changes.rs`), which already coalesces a burst into one
  `entity.changed` per window; `agent_of_tab` for the owner.
- **Never** invents a push event (the spec: invalidation stays content-free);
  never sends the snapshot over the wire on the change; never keeps the pump
  alive past the activity stream's close.

---

## Wire

### `surfaces` on the agent digest

- **Layer** wire
- **File** the payloads `bridge/src/app.rs` builds through `plan_view` /
  `run_view` / `agent_digests` at `DigestScope::Detail`
- **Responsibility** Carry the snapshot, additively, on detail payloads.
- **Which payloads carry surfaces** The four the spec pins — `branch.get`,
  `issue.get`, `run.get`, `plan.get` — AND the mutation answers that return the
  very same detail view (`thread.post`, which sends `MUTATION_THREAD_PAGE`,
  `spa/src/core/thread.js:150`; `agent.start` and the other run/issue mutations
  listed under `DigestScope`). The mutations are in deliberately: the rail
  repaints from the answer to its own send, and an answer that dropped the pills
  would blank the viewer for one tick after every message. The payloads are the
  same bounded detail views either way, so this adds no unbounded poll.
- **Reuses** the existing `agents[]` digest array — no new RPC, no new event.
- **Never** appears on a list-shaped payload — `board.list`, `plan.list`,
  `issue.list`, the archived list, `agent.list`, `agent.add`, `agent.remove`;
  never sends a key for a kind with no content; never sends `surfaces: {}`.

### `parent_sequence` on a thread event item

- **Layer** wire
- **File** `bridge/src/thread.rs` item serialization
- **Responsibility** Name the tool-call row this row folds under.
- **Reuses** the `sequence` a call's row already carries and the SPA's cache
  already keys on (`spa/src/core/thread.js`, `createThreadCache`).
- **Never** carries the harness's own `parent_tool_use_id`; never appears on a
  row with no parent.

---

## SPA

The three files are named `agentSurfaces*` rather than `surfaces*` because the
SPA already uses "surface" for something else: `spa/src/core/surfaceTabs.js` is
the agent PTY body, and tabshell and inbox call every worktree-backed view a
surface. These mirror the `agentRail.js` / `agentRailModel.js` pair they sit
beside, and match the export `mountAgentSurfaces`.

### `agentSurfacesModel.js`

- **Layer** spa
- **File** `spa/src/core/agentSurfacesModel.js` (new)
- **Responsibility** Every pure function from one agent digest to what the
  reader sees: which pills exist and what each says, which one is open, the
  rows of each kind, and the actions a row offers.
- **Reuses** the module shape of `spa/src/core/consoleModel.js` and
  `spa/src/core/agentRailModel.js` — pure, storage injected, no DOM;
  `workingClock` (`spa/src/core/agentRailModel.js:220`) to render an agent
  row's `duration_ms`.
- **Never** touches the DOM, `App.call`, or `localStorage` except through the
  injected storage object; never assumes a kind is present; never claims
  anything about an unrecognised `state` token.
- **Exports, and the boundaries between them**
  - `surfacePills(surfaces)` — one descriptor per kind that has content:
    `{ kind, label, count, live }`. `live` is true while anything in that kind
    is running. A kind with no content yields no pill. This is the ONLY place
    a count or a live dot is computed.
  - `openSurfaceKind(surfaces, wanted)` — the pill that is open, clamped to one
    that still exists, `null` for none. The mirror of
    `agentRailModel.js`'s `selectAgentId`.
  - `agentRows(agents)` — **the one agent normaliser.** The shared `agent` entry
    list normalised to what one row draws, each row carrying a `key`. It is
    exported in its own right rather than reachable only through
    `surfaceRows`, because the workflow viewer's right-hand pane paints agents
    that arrive nested under a phase and must go through the SAME arm as the
    subagent list — otherwise two differently-shaped inputs reach
    `agentRowHtml` and two different rules assign keys.
  - `surfaceRows(kind, surfaces)` — **the one normaliser, four arms**, keyed by
    the top-level kind. Every kind's entries come out of here as rows, and
    every row carries a `key`: the entry's `id` when it has one,
    `${kind}-${index}` when it does not. This is where defensive keying lives,
    so no viewer and no renderer ever computes a key and `patchList` never sees
    a duplicate. The `subagents` arm is `agentRows(surfaces.subagents)` — a
    delegation, not a second copy.
  - `workflowPhases(workflow, selectedIndex)` — returns
    `{ phases, agents }`: the phase list with done counts, and the selected
    phase's agents ALREADY through `agentRows`. The viewer never sees a raw
    `phases[].agents` array, so a phase's agents are keyed by the same rule as
    every other row — including the two id-less queued agents a fresh workflow
    carries (`bridge/tests/fixtures/claude-stream/workflow.jsonl:40`), which
    would otherwise collide and throw at `spa/src/core/patchList.js:134`.
  - `surfaceStateMark(kind, state)` — the state token → `{ mark, label }`
    lookup for every kind (agent, workflow, shell, checklist). One table. No
    kind renderer maps a state itself, and this returns a mark NAME, never a
    glyph — the glyph belongs to `outcomeMark.js`.
  - `rowActions(kind, entry)` — the canned messages a row's menu offers, shaped
    as `menuButtonMarkup` options plus the message:
    `{ id, label, description, message }`. Data only; nothing here sends. The
    shape is deliberate — render hands the array straight to
    `menuButtonMarkup` (`spa/src/core/splitButton.js:36`) with no adapter, and
    `agentSurfaces.js` finds the message by `id`.
  - `readOpenSurface(key, storage)` / `writeOpenSurface(key, kind, storage)` —
    the per-agent open pill, remembered the way `consoleModel.js` remembers a
    console size per work item. `key` is entity id plus agent id.

### `agentSurfacesRender.js`

- **Layer** spa
- **File** `spa/src/core/agentSurfacesRender.js` (new)
- **Responsibility** Pure markup: the pill row, the viewer region's frame, and
  the four kind renderers.
- **Reuses** `esc` (`core/text.js`); `menuButtonMarkup`
  (`core/splitButton.js`) for the row action menu, passed the array
  `rowActions` returns; `outcomeMarkHtml` (below) for every state mark;
  `.sdot.sdot-working` (`spa/src/styles/shell.css:188`) for a pill's live dot,
  which is the dot the status line already pulses; `aria-pressed` on the pill
  button, the pressed idiom `thread.js`'s option chips already use
  (`spa/src/core/thread.js:666`); the `<details>` fold idiom `thread.js` uses,
  so `domPatch.js` leaves a reader-opened fold alone across a repaint.
- **Never** queries the document, attaches a handler, or calls the bridge;
  never renders a mark it looked up itself; never computes a key (rows arrive
  keyed from `surfaceRows` / `agentRows`).
- **Exports**
  - `surfacePillsHtml(pills, openKind)` — the toggle row. Pressed state is
    `aria-pressed` on a button, not a rebuild.
  - `agentRowHtml(row)` — **the one row renderer for both viewers.** Label,
    model, state mark, last tool, tokens/calls/duration, result or error, and
    the action menu. Workflow agents and subagents are one shape (spec:
    "One shape means one row renderer"), so there is exactly one of these, and
    it takes only what `agentRows` produced. It emits `data-call-sequence` when
    the row has one and nothing at all when it does not — which is how the
    subagent viewer gets a pressable row and the workflow viewer gets
    byte-identical markup without a second renderer.
  - `workflowViewerHtml(workflow, phases, agents)` — phases on the left, the
    selected phase's agents on the right, each agent through `agentRowHtml`.
    Both arguments come out of `workflowPhases`.
  - `subagentViewerHtml(rows)` — the same rows through the same `agentRowHtml`;
    the press target is the `data-call-sequence` attribute that renderer
    already emits.
  - `shellViewerHtml(rows)` — one row per shell, its tail in a `<pre>` inside
    the row's fold.
  - `checklistViewerHtml(rows)` — one line per item with its state mark.
- **Pinned by** a test that `agentRowHtml(agentRows([entry])[0])` is
  byte-identical whether `entry` came from `workflows[].phases[].agents` or
  from `subagents`, and that two id-less workflow agents paint without a throw.

### `agentSurfaces.js`

- **Layer** spa
- **File** `spa/src/core/agentSurfaces.js` (new)
- **Responsibility** Mount the pill row and the viewer region, paint them from
  a digest, and wire the three gestures: toggling a pill, choosing a workflow
  phase, and a row's action.
- **Reuses** `patchList` (`core/patchList.js`) to paint every viewer list —
  keyed by `row.key` and by nothing else, for all four kinds, so the throw at
  `spa/src/core/patchList.js:134` is unreachable by construction;
  `mountSplitMenu` (`core/splitButton.js`) for the row menus; `notifyError`
  (`core/notify.js`) for a refused action.
- **The workflow viewer is two keyed lists, not one** the phase list and the
  selected phase's agent list are separate `patchList` containers, both keyed
  by `row.key`. Nothing paints a phase's agents as part of a phase row, so a
  phase changing its selection does not rebuild the agent rows under it.
- **Never** polls, calls `App.call` directly, or knows which entity it is on:
  it is handed `onSendMessage(text)` by the rail and posts through the rail's
  one send path (`agentRail.js`'s `post`), so an action is an ordinary message
  with ordinary adoption and waking; never derives a key itself.
- **Shape** `mountAgentSurfaces(host, { key, onSendMessage, onOpenThreadItem })`
  → `{ set(surfaces), dispose() }`. `set` is called from the rail's paint with
  the open agent's digest; a `set` that would change nothing paints nothing.

### the pill row and the viewer region, in the rail

- **Layer** spa
- **File** `spa/src/core/agentRail.js`, `spa/src/styles/shell.css`
- **Responsibility** Give the two regions their place: the viewer above the
  pills, the pills between the pinned status line and the composer, all inside
  the existing `.rail-composer` block.
- **Reuses** `composerRowHtml`'s existing structure, which already pins
  `#rail-status` above the composer; the sheet's `.rail-status` /
  `.rail-composer` rules as the neighbours the new rules sit beside.
- **Never** rebuilds the composer to paint a pill — the send, the draft and the
  focus survive every surface repaint, exactly as `syncComposer` already
  guarantees for the interrupt shape; never lets the viewer grow unbounded (it
  is height-capped with its own scroll).

### `outcomeMark.js`

- **Layer** spa
- **File** `spa/src/core/outcomeMark.js` (new; lifted out of `thread.js`)
- **Responsibility** **The one glyph table.** It maps a mark NAME to
  `{ glyph, tone }` and renders it, and it owns no vocabulary of its own — no
  tool tokens, no state tokens, no labels.
- **Reuses** — it IS the reuse: the glyph/tone/`role="img"` markup
  `thread.js`'s `toolOutcomeHtml` writes today moves here whole.
- **Exports** `outcomeMarkHtml(markName, label)` — the label is the caller's,
  because a mark means different things to different callers and only the
  caller knows which.
- **Never** invents a mark for a name it does not know: an unrecognised name
  renders no mark at all, which is the client's-direction reading the thread
  already uses for a tool outcome from a newer daemon.
- **The two vocabularies that feed it, and where each lives**
  - `thread.js` keeps a three-entry tool-outcome map — `ok`, `error`,
    `unanswered` → `{ mark, label }` — because those labels are tool-specific
    ("The tool answered", `spa/src/core/thread.js:783`) and cannot label a
    subagent's `done`. `toolOutcomeHtml` becomes a lookup in it plus a call to
    `outcomeMarkHtml`.
  - `agentSurfacesModel.js`'s `surfaceStateMark(kind, state)` returns
    `{ mark, label }` for the four surface kinds.

  Two vocabulary maps, one glyph table, one renderer.
- **What does NOT move** `EVENT_META` and `OUTCOME_META` (`thread.js:18`,
  `thread.js:687`) stay exactly where they are. A message outcome is a
  different record with a different vocabulary and its own icons; folding it in
  here would be a third vocabulary in the glyph table's file.

### the thread fold for parent-linked rows

- **Layer** spa
- **File** `spa/src/core/thread.js`
- **Responsibility** Draw a row carrying `parent_sequence` inside its parent
  tool-call row's existing fold, and never as a row of its own.
- **Reuses** `timelineHtml` (the one place items become rows), `activityHtml`'s
  `<details>` body, and `foldActivityRuns` — a parented child is removed from
  the top-level entry list before runs are folded, so a run's count and its
  ticker line stay counts of what the reader can actually see.
- **Never** renders a child twice; never folds a row under a parent that is not
  in the window (such a row renders flat, and reaches its parent when the
  reader scrolls back far enough to fetch it); never changes what an activity
  run is.
- **Needs** activity rows to carry `data-sequence`, which they do not today —
  that is what `revealThreadSequence(scroller, sequence)` (new export in
  `thread.js`) finds when the subagent viewer asks the thread to scroll to a
  spawning call and open it, reading the sequence off the pressed row's
  `data-call-sequence`.

---

## The boundaries, stated once

1. **Snapshot versus rows.** `SurfaceLedger` owns the snapshot; `ProtocolReader`
   owns the rows. They read the same lines and never write each other's
   output. `ProtocolState.tasks` stays the single authority for
   `live_status()` — surfaces are never a second answer to "is this agent
   working". Both live on `ProtocolState`, under one lock, as two fields with
   two jobs.
2. **Parsing versus routing.** `adk.rs` matches on `system` subtype and hands
   the value over; `surfaces.rs` matches on `task_type`. Neither does the
   other's match. `shell_tail.rs` reads files and parses no protocol at all.
   The harness-token vocabulary (`task_status_is_terminal` /
   `task_status_failed`) lives once, in `adk.rs`, and `surfaces.rs`'s two
   tables (`wire_task_state`, `wire_agent_state`) are the only mapping from it
   to a wire token.
3. **Signal versus content.** The watch channel says *moved*; the digest
   carries *what*. Nothing about a surface rides an `entity.changed` push. Only
   `SurfaceLedger` decides *moved*, and both the stdout reader and the tail
   poller say so through the one `SurfaceRevision::bump()`.
4. **Detail versus list.** Surfaces exist on detail payloads and nowhere else,
   decided by an explicit `DigestScope` argument at every `plan_view` /
   `run_view` / `agent_digests` call site — never inferred from `ThreadDetail`,
   never inferred from which caller is asking. A new handler that forgets to
   choose does not compile.
5. **Pure versus mounted.** `agentSurfacesModel.js` answers questions,
   `agentSurfacesRender.js` writes strings, `agentSurfaces.js` touches the DOM
   and the rail. A function in the wrong one of those three is the failure this
   document is written to prevent. Keying is a model question, so it belongs to
   `surfaceRows` / `agentRows` — including the agents nested inside a workflow
   phase, which reach the renderer only through `workflowPhases`.
6. **One row, one mark, one memory, one send.** Both viewers share
   `agentRowHtml` over rows both got from `agentRows`; all four kinds share
   `surfaceStateMark` + `outcomeMarkHtml`; the open pill is remembered by one
   storage pair; every row action is an ordinary message through the rail's
   existing `post`.
7. **One pairing map.** `Tab::call_sequences` is the only call-id-to-sequence
   record in the app layer, and it answers `parent_sequence`, `call_sequence`
   and "did this call ever get answered". The harness layer holds `tool_use_id`
   and never a sequence; `AgentSurfaces::wire_value` writes the JSON;
   `agent_digest` supplies the one closure that joins them.
8. **Read-only.** Nothing in the SPA or the bridge stops a workflow, kills a
   shell or checks a checklist item. A row action asks the agent; the agent
   decides.

## What must be shared, so no two kinds implement it

| Thing | Lives in | Used by |
| --- | --- | --- |
| the `agent` entry shape | `SurfaceAgent` (bridge), `agentRows` (spa) | workflows, subagents |
| one agent row's markup | `agentRowHtml` | workflow viewer, subagent viewer |
| a harness token → terminal / failed | `task_status_is_terminal` + `task_status_failed` (`adk.rs`, `pub(crate)`) | `ended_summary`, `wire_task_state` |
| a harness token → a wire state | `wire_task_state` / `wire_agent_state` (`surfaces.rs`) | all four kind parsers |
| a state token → a mark name | `surfaceStateMark` (spa), tool-outcome map (`thread.js`) | all four kinds, tool-call rows |
| a mark name → a glyph | `outcomeMarkHtml` | both vocabulary maps |
| pill counts and the live dot | `surfacePills` | all four kinds |
| which pill is open, remembered | `openSurfaceKind` + `readOpenSurface` / `writeOpenSurface` | all four kinds |
| a row's key | `surfaceRows` / `agentRows` | all four viewers' `patchList` paints |
| a row's action menu | `rowActions` + `menuButtonMarkup` + `mountSplitMenu` | all four kinds |
| keyed painting | `patchList` | all four viewers |
| call id → thread sequence, answered or not | `Tab::call_sequences` | `parent_sequence`, `call_sequence`, the pump's `Closed` arm |
| the snapshot → wire JSON | `AgentSurfaces::wire_value` | `agent_digest` |
| which payloads may carry surfaces | `DigestScope` | every `plan_view` / `run_view` / `agent_digests` call site |
| the running shells' output paths | `SurfaceLedger::running_shell_outputs` | `ShellTailPoller` |
| did the snapshot move | `SurfaceLedger`'s booleans → `SurfaceRevision::bump()` | stdout reader, tail poller |
| the change that stales a detail | `note_entity_changed` → `ChangeBus` | every kind's revision bump |

## Where this deviates from the spec

One deviation, reported to the spec's owner rather than taken silently:

- **`agent.id` cannot be required.** The spec's shared `agent` entry marks `id`
  required. A queued workflow agent has no `agentId` in the stream
  (`bridge/tests/fixtures/claude-stream/workflow.jsonl:40`, index 2). Dropping
  such an entry would make the pill count and the phase done-count lie, so
  `apply_workflow` synthesises `"{workflow_task_id}:{index}"` and the row takes
  its real id once, when the agent starts. Nothing else in the system invents
  an id. If the spec would rather the wire carried an explicit
  `id_is_provisional`, that is the spec's call to make.

## Risks this shape is deliberately taking

- `agent_digests` today answers for the board rows AND the detail views out of
  one function. Adding `surfaces` without splitting the decision would ship an
  unbounded-ish snapshot on a 2.5s board poll for every entity on screen. The
  explicit `DigestScope` argument is what keeps the leak from being an
  accident: it cannot be got wrong by a caller who simply did not think about
  it, because there is no default.
- Surfaces ride mutation answers as well as the four `.get`s. That is a wider
  wire surface than the spec's four RPCs name, taken on purpose so the rail can
  repaint the pills from the answer to its own send. All of them are bounded
  detail views; none of them is a poll.
- The activity broadcast's payload type changes from `AgentActivity` to
  `ActivityReport`. It is a small ripple (`session.rs`, `adk.rs`,
  `spawn_activity_pump`, `record_activity`) but it is a ripple.
- Moving `open_calls` off the tokio task and onto `Tab` makes it session-lived
  rather than answer-lived, so it grows with the session's tool calls. That is
  the price of `call_sequence`: a subagent's surface entry outlives its Agent
  call's answer. It is bounded by the session and cleared in the pump's
  `Closed` arm, and the `answered` flag is what keeps the `Closed` arm from
  re-resolving calls that already have an answer.
- `bridge/src/harness/adk.rs` is already 3.5k lines. Putting four more parsers
  in it is the easy move and the wrong one; `surfaces.rs` is where they go, and
  the file read they need is `shell_tail.rs`.
- The shell tail path comes out of a tool result's text. It is a path the
  harness named for a file the harness is writing — it must be treated as
  untrusted input, read bounded, and never followed outside the reads
  `ShellTail` makes.
- `patchList` throws on duplicate keys. `surfaceRows` and `agentRows` are the
  only places a key is assigned, and they fall back to `${kind}-${index}` for
  an entry with a missing or repeated `id`, so a malformed workflow, shell or
  checklist entry degrades to a row that loses identity across a repaint rather
  than to a thrown paint.
- None of the workflow fields are documented by Anthropic. Every parser is
  pinned to `bridge/tests/fixtures/claude-stream/` and every field is optional;
  a CLI update is expected to move them.

## Gates this work is held to

- Bridge: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt`, run in `bridge/`.
- SPA: `npm test` (vitest), run in `spa/`. **There is no `lint` script in
  `spa/package.json`** — the scripts are `dev`, `build`, `preview`, `test`,
  `test:watch`. Either a lint script is added there before implementation
  starts or the SPA gate is the test half only. Whoever picks up the
  implementation must not report a lint run that cannot have happened.
