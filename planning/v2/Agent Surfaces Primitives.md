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

### `SurfaceLedger`

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new)
- **Responsibility** The mutable state one session's stream builds
  `AgentSurfaces` in, and the only place a stream-json line is turned into
  surface state.
- **Reuses** `serde_json::Value` field reads in the manner of `adk.rs`'s
  `task_description` / `task_status_is_terminal`; the wholesale-replace
  discipline `read_task_roster` already uses for the background-task roster.
- **Never** touches the filesystem, the broadcast channel or the app state;
  never merges a `workflow_progress` array (it is a full snapshot — replace);
  never claims a `state` token it does not recognise.
- **Shape** One entry point per line class, so `adk.rs` matches on `subtype`
  and this matches on `task_type`:
  `read_task_event(subtype, &Value) -> bool`, `read_tool_call(name, &Value)`,
  `read_tool_answer(call_id, &Value)`, `snapshot() -> Option<AgentSurfaces>`.
  Each returns whether the snapshot moved; that boolean is the ONLY trigger
  for a revision bump.

### the four kind parsers

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new), private to `SurfaceLedger`
- **Responsibility** One function per kind, each owning the fields the spec's
  "Where the data comes from" section pins for it:
  `apply_workflow` (`local_workflow` + `workflow_progress`),
  `apply_subagent` (`local_agent`), `apply_shell` (`local_bash` + the launch
  path recorded off the Bash result), `apply_checklist` (`TaskCreate` /
  `TaskUpdate` / `TodoWrite`).
- **Reuses** `SurfaceAgent`, so workflow agents and subagents are one shape
  built by one constructor; the terminal-status vocabulary already written in
  `adk.rs::task_status_is_terminal`.
- **Never** duplicates another kind's field mapping; never mints an activity
  row (rows are the reader's business, snapshots are this one's); never parses
  a field as required — every field on this wire is optional and pinned to a
  fixture.

### `ShellTail`

- **Layer** bridge
- **File** `bridge/src/harness/surfaces.rs` (new)
- **Responsibility** Read the last 20 lines of one shell's output file and the
  `[exited with code N]` marker at its end.
- **Reuses** nothing; it is a bounded file read with a cap named once.
- **Never** reads a path the harness did not name in its own Bash result;
  never reads the whole file into memory; never runs while no shell is
  running.

### `ShellTailPoller`

- **Layer** bridge
- **File** `bridge/src/harness/adk.rs` (spawned beside the stdout reader)
- **Responsibility** Tail every running shell once a second and bump the
  surface revision when a tail changed, for as long as the ledger holds a
  running shell and no longer.
- **Reuses** `ShellTail`; the `std::thread::spawn` reader pattern `AdkSession::spawn`
  already uses for stdout and stderr.
- **Never** runs on an empty shell set; never bumps the revision on a read that
  changed nothing; never outlives the session's stdout reader.

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

### `agent_digest` surfaces field

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** Attach `surfaces` to an agent digest, on entity detail
  payloads only.
- **Reuses** `AppState::agent_digest` / `agent_digests`, which already answer
  for `branch.get`, `issue.get`, `run.get`, `plan.get`, `agent.list` AND the
  board's branch/issue rows — so the detail-versus-list decision becomes an
  explicit parameter on those two functions rather than a second digest
  builder.
- **Never** rides `board.list` or `plan.list`; never re-reads the session more
  than once per digest; never ships a kind with nothing in it.

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

### `record_activity` parent pairing

- **Layer** bridge
- **File** `bridge/src/app.rs`
- **Responsibility** Turn an `ActivityReport`'s `parent_call_id` into the
  spawning row's `parent_sequence` on the row it mints.
- **Reuses** the pump's existing `open_calls: HashMap<String, u64>` map — the
  call id against the sequence of the row it minted — which is already the only
  record of that pairing. No second map.
- **Never** holds a parent link for a call the map has forgotten; never mints a
  parented row for one of Build's own MCP calls (those are silent by
  construction).

---

## Wire

### `surfaces` on the agent digest

- **Layer** wire
- **File** the payloads `bridge/src/app.rs` builds for `branch.get`,
  `issue.get`, `run.get`, `plan.get`, `agent.list`
- **Responsibility** Carry the snapshot, additively, on detail payloads.
- **Reuses** the existing `agents[]` digest array — no new RPC, no new event.
- **Never** appears on a list payload; never sends a key for a kind with no
  content; never sends `surfaces: {}`.

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

### `surfacesModel.js`

- **Layer** spa
- **File** `spa/src/core/surfacesModel.js` (new)
- **Responsibility** Every pure function from one agent digest to what the
  reader sees: which pills exist and what each says, which one is open, the
  rows of each kind, and the actions a row offers.
- **Reuses** the module shape of `spa/src/core/consoleModel.js` and
  `spa/src/core/agentRailModel.js` — pure, storage injected, no DOM.
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
  - `surfaceAgentRows(entries)` — the shared `agent` entry list, normalised to
    what one row draws, for BOTH the workflow viewer and the subagent viewer.
  - `workflowPhases(workflow, selectedIndex)` — the phase list with done counts
    and which phase's agents are showing.
  - `surfaceStateMark(kind, state)` — the state token → mark lookup for every
    kind (agent, workflow, shell, checklist). One table. No kind renderer maps
    a state itself.
  - `rowActions(kind, entry)` — the canned messages a row's menu offers, as
    `{ id, menuLabel, description, message }`. Data only; nothing here sends.
  - `readOpenSurface(key, storage)` / `writeOpenSurface(key, kind, storage)` —
    the per-agent open pill, remembered the way `consoleModel.js` remembers a
    console size per work item. `key` is entity id plus agent id.

### `surfacesRender.js`

- **Layer** spa
- **File** `spa/src/core/surfacesRender.js` (new)
- **Responsibility** Pure markup: the pill row, the viewer region's frame, and
  the four kind renderers.
- **Reuses** `esc` (`core/text.js`); `menuButtonMarkup`
  (`core/splitButton.js`) for the row action menu; `outcomeMarkHtml`
  (below) for every state mark; the `<details>` fold idiom `thread.js` uses,
  so `domPatch.js` leaves a reader-opened fold alone across a repaint.
- **Never** queries the document, attaches a handler, or calls the bridge;
  never renders a mark it looked up itself.
- **Exports**
  - `surfacePillsHtml(pills, openKind)` — the toggle row. Pressed state is an
    attribute on a button, not a rebuild.
  - `agentRowHtml(row)` — **the one row renderer for both viewers.** Label,
    model, state mark, last tool, tokens/calls/duration, result or error, and
    the action menu. Workflow agents and subagents are one shape (spec:
    "One shape means one row renderer"), so there is exactly one of these.
  - `workflowViewerHtml(workflow, phases)` — phases on the left, the selected
    phase's agents on the right, each agent through `agentRowHtml`.
  - `subagentViewerHtml(rows)` — the same rows, each carrying its
    `call_sequence` so a press can reach the thread.
  - `shellViewerHtml(shells)` — one row per shell, its tail in a `<pre>` inside
    the row's fold.
  - `checklistViewerHtml(items)` — one line per item with its state mark.

### `surfaces.js`

- **Layer** spa
- **File** `spa/src/core/surfaces.js` (new)
- **Responsibility** Mount the pill row and the viewer region, paint them from
  a digest, and wire the three gestures: toggling a pill, choosing a workflow
  phase, and a row's action.
- **Reuses** `patchList` (`core/patchList.js`) to paint every viewer list, so a
  growing workflow keeps row identity and the scroll inside an open fold;
  `mountSplitMenu` (`core/splitButton.js`) for the row menus; `notifyError`
  (`core/notify.js`) for a refused action.
- **Never** polls, calls `App.call` directly, or knows which entity it is on:
  it is handed `onSendMessage(text)` by the rail and posts through the rail's
  one send path (`agentRail.js`'s `post`), so an action is an ordinary message
  with ordinary adoption and waking.
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
- **Responsibility** One glyph + tone + screen-reader label renderer for a
  state token, and the tables of tokens it knows.
- **Reuses** — it IS the reuse: `thread.js`'s `TOOL_OUTCOME_MARKS` and
  `toolOutcomeHtml` move here, `toolOutcomeHtml` becomes a call to it, and
  every surface kind marks its state through the same function.
- **Never** invents a mark for a token it does not know: an unrecognised token
  renders no mark at all, which is the client's-direction reading the thread
  already uses for a tool outcome from a newer daemon.

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
  spawning call and open it.

---

## The boundaries, stated once

1. **Snapshot versus rows.** `SurfaceLedger` owns the snapshot; `ProtocolReader`
   owns the rows. They read the same lines and never write each other's
   output. `ProtocolState.tasks` stays the single authority for
   `live_status()` — surfaces are never a second answer to "is this agent
   working".
2. **Parsing versus routing.** `adk.rs` matches on `system` subtype and hands
   the value over; `surfaces.rs` matches on `task_type`. Neither does the
   other's match.
3. **Signal versus content.** The watch channel says *moved*; the digest
   carries *what*. Nothing about a surface rides an `entity.changed` push.
4. **Detail versus list.** Surfaces exist on entity detail payloads and nowhere
   else, decided by one parameter on `agent_digest` rather than by which caller
   is asking.
5. **Pure versus mounted.** `surfacesModel.js` answers questions,
   `surfacesRender.js` writes strings, `surfaces.js` touches the DOM and the
   rail. A function in the wrong one of those three is the failure this
   document is written to prevent.
6. **One row, one mark, one memory, one send.** Both viewers share
   `agentRowHtml`; all four kinds share `surfaceStateMark` + `outcomeMarkHtml`;
   the open pill is remembered by one storage pair; every row action is an
   ordinary message through the rail's existing `post`.
7. **Read-only.** Nothing in the SPA or the bridge stops a workflow, kills a
   shell or checks a checklist item. A row action asks the agent; the agent
   decides.

## What must be shared, so no two kinds implement it

| Thing | Lives in | Used by |
| --- | --- | --- |
| the `agent` entry shape | `SurfaceAgent` (bridge), `surfaceAgentRows` (spa) | workflows, subagents |
| one agent row's markup | `agentRowHtml` | workflow viewer, subagent viewer |
| a state token → mark | `surfaceStateMark` + `outcomeMarkHtml` | all four kinds, tool-call rows |
| pill counts and the live dot | `surfacePills` | all four kinds |
| which pill is open, remembered | `openSurfaceKind` + `readOpenSurface` / `writeOpenSurface` | all four kinds |
| a row's action menu | `rowActions` + `menuButtonMarkup` + `mountSplitMenu` | all four kinds |
| keyed painting | `patchList` | all four viewers |
| the change that stales a detail | `note_entity_changed` → `ChangeBus` | every kind's revision bump |

## Risks this shape is deliberately taking

- `agent_digests` today answers for the board rows AND the detail views out of
  one function. Adding `surfaces` without splitting the decision would ship an
  unbounded-ish snapshot on a 2.5s board poll for every entity on screen.
- The activity broadcast's payload type changes from `AgentActivity` to
  `ActivityReport`. It is a small ripple (`session.rs`, `adk.rs`,
  `spawn_activity_pump`, `record_activity`) but it is a ripple.
- `bridge/src/harness/adk.rs` is already 3.5k lines. Putting four more parsers
  in it is the easy move and the wrong one; `surfaces.rs` is where they go.
- The shell tail path comes out of a tool result's text. It is a path the
  harness named for a file the harness is writing — it must be treated as
  untrusted input, read bounded, and never followed outside the reads
  `ShellTail` makes.
- `patchList` throws on duplicate keys. A workflow, shell or checklist entry
  with a missing or repeated `id` must be keyed defensively before it reaches a
  paint.
- None of the workflow fields are documented by Anthropic. Every parser is
  pinned to `bridge/tests/fixtures/claude-stream/` and every field is optional;
  a CLI update is expected to move them.
