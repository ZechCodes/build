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
and this document is wrong.

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
- **Reuses** `serde_json::Value` field reads in the manner of `adk.rs`'s
  `task_description` / `task_status_is_terminal`; the wholesale-replace
  discipline `read_task_roster` already uses for the background-task roster;
  `ShellTail` as a plain value handed in from `shell_tail.rs`.
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
  poller alike, both bumping through the one
  `AdkSession::note_surfaces_moved()`, and only on `true`.

  The one call-id map the ledger keeps is
  `pending_checklist_creates: HashMap<String, (String, String)>` — a `TaskCreate`
  call's subject and description, held only until its answer names the task id,
  and drained on answer. It is named for that job and holds nothing else.

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
  built by one constructor; the terminal-status vocabulary already written in
  `adk.rs::task_status_is_terminal`.
- **Never** duplicates another kind's field mapping; never mints an activity
  row (rows are the reader's business, snapshots are this one's); never parses
  a field as required — every field on this wire is optional and pinned to a
  fixture.
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
- **File** `bridge/src/harness/adk.rs` (spawned beside the stdout reader)
- **Responsibility** Tail every running shell once a second and hand each tail
  to `SurfaceLedger::read_shell_tail`, for as long as the ledger holds a
  running shell and no longer.
- **Reuses** `ShellTail`; `AdkSession::note_surfaces_moved()`, the same bump the
  stdout reader uses; the `std::thread::spawn` reader pattern `AdkSession::spawn`
  already uses for stdout and stderr.
- **Never** runs on an empty shell set; never decides whether a tail moved the
  snapshot (the ledger's boolean decides, and the poller bumps only on `true`);
  never writes a shell's state or exit code; never outlives the session's
  stdout reader.

### `AgentSession::surfaces` / `AgentSession::surfaces_changed`

- **Layer** bridge
- **File** `bridge/src/harness/session.rs`
- **Responsibility** The capability accessor — this session's snapshot, or
  `None` — and the watch channel that says the snapshot moved.
- **Reuses** the exact shape of `AgentSession::terminal` and
  `AgentSession::activity`: a defaulted method returning `None`, so a carrier
  that offers nothing writes not one line about surfaces.
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
  thread sequence of the row that call minted. It is the single answer to both
  "which row does this child fold under" (`parent_sequence`) and "which row
  spawned this subagent" (`call_sequence`).
- **Reuses** the pairing the activity pump already computes — today as
  `open_calls`, a local inside the tokio task in `spawn_activity_pump`
  (`bridge/src/app.rs:18363`). Moving it onto `Tab` changes where it lives, not
  what it holds.
- **Never** a second map: `record_activity` reads and writes this one, and
  `agent_digest` reads this one. Nothing else keeps a call-id-to-sequence
  record in the app layer.
- **Lifetime** Kept for the session's life rather than drained on answer —
  a subagent's surface entry outlives the Agent call's answer, so a map that
  forgot on answer could not stamp `call_sequence`. It is drained in the pump's
  `Closed` arm, beside the `tab.live = false` the pump already writes there, so
  a dead session's map dies with it.

### `record_activity` parent pairing

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** Turn an `ActivityReport`'s `parent_call_id` into the
  spawning row's `parent_sequence` on the row it mints, and record every call
  it mints a row for into the tab's `call_sequences`.
- **Reuses** `Tab::call_sequences` — the one map, read for the parent and
  written for the newly minted call. The pump no longer carries a local
  `open_calls` and passes no map.
- **Never** holds a parent link for a call the map has forgotten; never mints a
  parented row for one of Build's own MCP calls (those are silent by
  construction).

### `agent_digest` surfaces field

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** Attach `surfaces` to an agent digest on entity detail
  payloads only, and stamp each subagent entry's `call_sequence` by looking its
  `spawning_call_id` up in the tab's `call_sequences` as it attaches them. The
  harness layer never learns what a sequence is; this is the one place the two
  halves meet.
- **Reuses** `ThreadDetail` (`bridge/src/thread.rs:898`) as the parameter on
  `agent_digest` / `agent_digests`, NOT a fresh boolean: `plan_view` and
  `run_view` already take it (`Digest` for `board.list` / `plan.list` at
  `bridge/src/app.rs:9647`, `9656`, `12708`, `12723`; `Full` / `Page` for the
  `.get`s) and already call `agent_digests` from inside
  (`bridge/src/app.rs:14223`, `14358`). `ThreadDetail::Digest` means no
  surfaces; anything else means surfaces. Where a two-variant `DigestScope`
  reads better at the call sites, it is derived from `ThreadDetail` ONCE at the
  top of `plan_view` / `run_view` and nowhere else. Also reuses
  `Tab::call_sequences` and `agent_of_tab`.
- **Never** rides `board.list` or `plan.list`; never lets a caller ask for a
  `Digest` thread with full surfaces (that combination is the board-poll leak
  the Risks section names, and making the parameter `ThreadDetail` is what
  makes it unsayable); never re-reads the session more than once per digest;
  never ships a kind with nothing in it.
- **The list-shaped answers keep no surfaces** `agent.list`, `agent.add`
  (`bridge/src/app.rs:8177`) and `agent.remove` (`bridge/src/app.rs:8275`)
  answer with the list-shaped digest. The SPA has no caller of `agent.list` at
  all.

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
- **File** the payloads `bridge/src/app.rs` builds for `branch.get`,
  `issue.get`, `run.get`, `plan.get` — the four the spec pins, and no others
- **Responsibility** Carry the snapshot, additively, on detail payloads.
- **Reuses** the existing `agents[]` digest array — no new RPC, no new event.
- **Never** appears on a list payload or on `agent.list` / `agent.add` /
  `agent.remove`; never sends a key for a kind with no content; never sends
  `surfaces: {}`.

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
  - `surfaceRows(kind, surfaces)` — **the one normaliser, four arms.** Every
    kind's entries come out of here as rows, and every row carries a `key`:
    the entry's `id` when it has one, `${kind}-${index}` when it does not. This
    is where defensive keying lives, so no viewer and no renderer ever computes
    a key and `patchList` never sees a duplicate. The agent arm is what used to
    be `surfaceAgentRows`: the shared `agent` entry list normalised to what one
    row draws, for BOTH the workflow viewer and the subagent viewer.
  - `workflowPhases(workflow, selectedIndex)` — the phase list with done counts
    and which phase's agents are showing.
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
  keyed from `surfaceRows`).
- **Exports**
  - `surfacePillsHtml(pills, openKind)` — the toggle row. Pressed state is
    `aria-pressed` on a button, not a rebuild.
  - `agentRowHtml(row)` — **the one row renderer for both viewers.** Label,
    model, state mark, last tool, tokens/calls/duration, result or error, and
    the action menu. Workflow agents and subagents are one shape (spec:
    "One shape means one row renderer"), so there is exactly one of these. It
    emits `data-call-sequence` when the row has one and nothing at all when it
    does not — which is how the subagent viewer gets a pressable row and the
    workflow viewer gets byte-identical markup without a second renderer.
  - `workflowViewerHtml(workflow, phases)` — phases on the left, the selected
    phase's agents on the right, each agent through `agentRowHtml`.
  - `subagentViewerHtml(rows)` — the same rows through the same `agentRowHtml`;
    the press target is the `data-call-sequence` attribute that renderer
    already emits.
  - `shellViewerHtml(rows)` — one row per shell, its tail in a `<pre>` inside
    the row's fold.
  - `checklistViewerHtml(rows)` — one line per item with its state mark.

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
   working".
2. **Parsing versus routing.** `adk.rs` matches on `system` subtype and hands
   the value over; `surfaces.rs` matches on `task_type`. Neither does the
   other's match. `shell_tail.rs` reads files and parses no protocol at all.
3. **Signal versus content.** The watch channel says *moved*; the digest
   carries *what*. Nothing about a surface rides an `entity.changed` push. Only
   `SurfaceLedger` decides *moved*, and both the stdout reader and the tail
   poller say so through the one `note_surfaces_moved()`.
4. **Detail versus list.** Surfaces exist on entity detail payloads and nowhere
   else, decided by the `ThreadDetail` the caller already passes rather than by
   which caller is asking or by a second boolean beside it.
5. **Pure versus mounted.** `agentSurfacesModel.js` answers questions,
   `agentSurfacesRender.js` writes strings, `agentSurfaces.js` touches the DOM
   and the rail. A function in the wrong one of those three is the failure this
   document is written to prevent. Keying is a model question, so it is
   `surfaceRows`'s.
6. **One row, one mark, one memory, one send.** Both viewers share
   `agentRowHtml`; all four kinds share `surfaceStateMark` + `outcomeMarkHtml`;
   the open pill is remembered by one storage pair; every row action is an
   ordinary message through the rail's existing `post`.
7. **One pairing map.** `Tab::call_sequences` is the only call-id-to-sequence
   record in the app layer, and it answers both `parent_sequence` and
   `call_sequence`. The harness layer holds `tool_use_id` and never a sequence;
   `agent_digest` is the one place the two are joined.
8. **Read-only.** Nothing in the SPA or the bridge stops a workflow, kills a
   shell or checks a checklist item. A row action asks the agent; the agent
   decides.

## What must be shared, so no two kinds implement it

| Thing | Lives in | Used by |
| --- | --- | --- |
| the `agent` entry shape | `SurfaceAgent` (bridge), `surfaceRows` agent arm (spa) | workflows, subagents |
| one agent row's markup | `agentRowHtml` | workflow viewer, subagent viewer |
| a state token → a mark name | `surfaceStateMark` (spa), tool-outcome map (`thread.js`) | all four kinds, tool-call rows |
| a mark name → a glyph | `outcomeMarkHtml` | both vocabulary maps |
| pill counts and the live dot | `surfacePills` | all four kinds |
| which pill is open, remembered | `openSurfaceKind` + `readOpenSurface` / `writeOpenSurface` | all four kinds |
| a row's key | `surfaceRows` | all four viewers' `patchList` paints |
| a row's action menu | `rowActions` + `menuButtonMarkup` + `mountSplitMenu` | all four kinds |
| keyed painting | `patchList` | all four viewers |
| call id → thread sequence | `Tab::call_sequences` | `parent_sequence`, `call_sequence` |
| did the snapshot move | `SurfaceLedger`'s booleans → `note_surfaces_moved()` | stdout reader, tail poller |
| the change that stales a detail | `note_entity_changed` → `ChangeBus` | every kind's revision bump |

## Risks this shape is deliberately taking

- `agent_digests` today answers for the board rows AND the detail views out of
  one function. Adding `surfaces` without splitting the decision would ship an
  unbounded-ish snapshot on a 2.5s board poll for every entity on screen.
  Passing `ThreadDetail` rather than a fresh boolean is what keeps the leak
  unsayable: the board's polls already pass `Digest` and cannot ask for more.
- The activity broadcast's payload type changes from `AgentActivity` to
  `ActivityReport`. It is a small ripple (`session.rs`, `adk.rs`,
  `spawn_activity_pump`, `record_activity`) but it is a ripple.
- Moving `open_calls` off the tokio task and onto `Tab` makes it session-lived
  rather than answer-lived, so it grows with the session's tool calls. That is
  the price of `call_sequence`: a subagent's surface entry outlives its Agent
  call's answer. It is bounded by the session and dropped in the pump's
  `Closed` arm.
- `bridge/src/harness/adk.rs` is already 3.5k lines. Putting four more parsers
  in it is the easy move and the wrong one; `surfaces.rs` is where they go, and
  the file read they need is `shell_tail.rs`.
- The shell tail path comes out of a tool result's text. It is a path the
  harness named for a file the harness is writing — it must be treated as
  untrusted input, read bounded, and never followed outside the reads
  `ShellTail` makes.
- `patchList` throws on duplicate keys. `surfaceRows` is the one place a key is
  assigned, and it falls back to `${kind}-${index}` for an entry with a missing
  or repeated `id`, so a malformed workflow, shell or checklist entry degrades
  to a row that loses identity across a repaint rather than to a thrown paint.
- None of the workflow fields are documented by Anthropic. Every parser is
  pinned to `bridge/tests/fixtures/claude-stream/` and every field is optional;
  a CLI update is expected to move them.
