# Agent Surfaces Spec

Status: locked design, 2026-09-01. Implementation tracked on this branch.

## What this is

An agent's conversation shows one row per thing the agent did. It says nothing
structured about the work the harness runs *beside* the conversation: multi-agent
workflows, subagents, background shells, and the agent's own checklist. The
stream the headless carrier already reads carries all of it and the bridge
throws it away.

This spec adds **surfaces**: one structured snapshot per agent, keyed by kind,
that the SPA renders as toggle pills under the chat composer. Clicking a pill
opens a viewer above the pills. A pill exists only when toggling it would show
something.

## Design rules

1. **Generic over harnesses.** The SPA knows four surface kinds and their
   shapes. It never sees a harness name. A harness adapter fills what it can;
   a missing kind means "this harness offers nothing here", never "empty".
2. **Snapshot, not rows.** A surface is state that is replaced wholesale on
   each read. It is not a new row kind in the thread. The thread's rows stay
   the record; surfaces are the live picture.
3. **Bounded.** A snapshot must stay small enough to ride an entity detail
   payload. Anything unbounded (a subagent's transcript) stays in the thread,
   paged as rows, and the surface links to it.
4. **Additive wire, read in the client's direction.** Unknown kinds and
   unknown state tokens claim nothing. The client renders only what it
   recognises.
5. **Invalidation stays content-free.** A surface change is `entity.changed`
   for the owning entity, the same push that already refetches the rail. No
   new push event.
6. **Read-only first.** The stream offers no control over a workflow, shell,
   or checklist item beyond interrupt. Row actions post a message to the agent
   asking for the change. The human decides, the agent acts.

## Shapes

All fields optional on the wire unless stated. Timestamps are epoch
milliseconds. `state` tokens are lowercase snake_case strings.

### `agent` entry (shared by workflows and subagents)

```
{
  id,              // harness id for the agent, required
  label,           // workflow label or subagent description, required
  model,           // resolved model id when known
  state,           // queued | running | done | failed
  started_at,
  duration_ms,
  tokens,
  tool_calls,
  last_tool: { name, summary },   // what it is doing right now
  result,          // one-line result preview once done
  error,           // one-line error once failed
  attempt,         // retry counter, 1-based
  call_sequence    // thread sequence of the tool-call row that spawned it, subagents only
}
```

One shape means one row renderer in the SPA for both viewers.

### `surfaces` on the agent digest

```
surfaces: {
  workflows: [ { id, name, description, state, phases: [ { title, agents: [agent] } ] } ],
  subagents: [ agent ],
  shells:    [ { id, description, state, exit_code, tail: [string] } ],
  checklist: [ { id, subject, description, state } ]
}
```

- `workflows[].state`: `running | done | failed`.
- `shells[].state`: `running | done | failed`. `tail` is the last 20 lines of
  the output file, no more.
- `checklist[].state`: `pending | in_progress | completed | blocked`, taken
  from the harness token verbatim when it is one of these.
- A kind with no content is omitted, not sent empty. A carrier that reports
  no surfaces at all omits the `surfaces` key.

## Where the data comes from (Claude Code stream-json, CLI 2.1.257)

Verified against captured runs. Fixtures live in
`bridge/tests/fixtures/claude-stream/`. None of the workflow fields are
documented by Anthropic; parse every field as optional and pin tests to the
fixtures.

### Workflows

- `system/task_started` with `task_type: "local_workflow"`, `workflow_name`,
  `description`, `tool_use_id`, and the script text in `prompt`.
- `system/task_progress` for that `task_id` sometimes carries
  `workflow_progress`: an array of `{type: "workflow_phase", index, title}`
  and `{type: "workflow_agent", index, label, phaseIndex, phaseTitle, agentId,
  model, state, startedAt, queuedAt, attempt, lastToolName, lastToolSummary,
  promptPreview, lastProgressAt, tokens, toolCalls, durationMs, resultPreview,
  error}`. Observed `state` values: `start`, `progress`, `done`. It is a full
  snapshot each time; replace, do not merge. Progress events without the
  array are usage ticks and change nothing.
- `system/task_updated` with `patch.status` and `system/task_notification`
  with `status`, `summary`, `usage` close it.
- Workflow agents never stream their messages. Their transcripts live on disk
  under the transcript directory named in the Workflow tool result. Reading
  those is out of scope for this pass.

### Subagents (Agent tool)

- `system/task_started` with `task_type: "local_agent"`, `subagent_type`,
  `spawn_depth`, `description`, `prompt`, `tool_use_id`. The `task_id` is the
  agent id.
- `system/task_progress` with `description` (current step), `last_tool_name`,
  `usage: {total_tokens, tool_uses, duration_ms}`.
- `system/task_notification` with `status`, `summary` (the agent's final
  answer), `usage`.
- The subagent's own messages stream inline as `assistant`/`user` messages
  whose `parent_tool_use_id` is the spawning Agent call's tool-use id. The
  bridge drops these today. They become thread rows carrying a parent link
  (the spawning call's sequence) so the thread folds them under that row.

### Background shells

- `system/task_started` with `task_type: "local_bash"`, `description`,
  `tool_use_id`.
- The Bash tool result that launched it names the output file in its text
  (`Output is being written to: <path>`) and carries `backgroundTaskId` in
  `tool_use_result`. Record the path at launch.
- No progress lines arrive while it runs. The output file grows on disk and
  ends with `[exited with code N]`. The bridge tails it once a second while any
  shell is running and bumps the surface revision when the tail changes.
- `system/task_notification` carries `status`, `output_file`, and a summary
  with the exit code.

### Checklist

- Tool calls `TaskCreate {subject, description}` answered by
  `tool_use_result.task {id, subject}`.
- `TaskUpdate {taskId, status, ...}` answered by
  `tool_use_result {taskId, statusChange: {from, to}}`.
- Older sessions may use `TodoWrite {todos: [{content, status, activeForm}]}`,
  which carries the whole list; treat it as a wholesale replace.

## Bridge

- The stream reader in `bridge/src/harness/adk.rs` already holds a private
  background-task roster. Extend that state into the four surfaces and route
  task events by `task_type`.
- Add a capability accessor on `AgentSession` returning the surfaces snapshot,
  parallel to the existing terminal and activity accessors, plus a revision
  watch channel. A session with no surfaces returns `None`.
- The per-session activity pump in `bridge/src/app.rs` selects on the revision
  channel and notes the owning entity as changed. Progress lines that mint no
  row still invalidate.
- `agent_digest` carries `surfaces` only on entity detail payloads
  (`branch.get`, `issue.get`, `run.get`, `plan.get`), never on the board list
  digests.
- The shell tail poller runs only while the shell set is non-empty.
- Subagent transcript rows: a new optional `parent_sequence` on thread events.
  The pump maps `parent_tool_use_id` to the spawning row's sequence through
  the pairing map it already keeps.

## SPA

- The chat surface bottom is the agent rail's composer block (`agentRail.js`),
  with the pinned status line above the composer. Pills render in a row
  between the status line and the composer. The viewer is a new region above
  the pills, height-capped with its own scroll.
- A pill shows a label, a count, and a live dot while anything in it is
  running. Pills are pressed-state toggles, one open at a time. The open choice
  is remembered per agent in local storage, the same way the console remembers
  its size per work item.
- The viewer is painted with the keyed reconciler (`patchList`) so a growing
  workflow keeps row identity and scroll position.
- Workflow viewer: phases on the left with done counts, the selected phase's
  agents on the right. Stacks on narrow widths.
- Subagent viewer: the same agent rows. Clicking one scrolls the thread to the
  spawning call row and opens it.
- Shell viewer: one row per shell with description and state, and a fold
  showing the tail in a preformatted block.
- Checklist viewer: a list with the existing outcome marks.
- Row action (all kinds): a menu that posts a canned message to the agent
  (for example, stop the named workflow). Uses the existing split-button menu.
- Thread: rows carrying `parent_sequence` fold under their parent call row
  inside the existing tool-call container.

## Out of scope for this pass

- Reading workflow agent transcripts from disk.
- Killing shells or workflows from the bridge.
- Surfaces for the Codex or PTY carriers.

## Revision 2026-09-02: grace-timed pills, one TUI toggle, conversation menu, startup line

Locked after the first pass shipped. Bridge unchanged; all four are SPA.

### Pills that count running work and linger sixty seconds

- For `shells` and `subagents` the pill count is the number of **running**
  entries, and the pill is shown while any of these hold: an entry is running;
  fewer than 60 seconds have passed since an entry was last seen running; the
  kind's viewer is open; fewer than 60 seconds have passed since that viewer
  was closed. Otherwise the pill is hidden. The grace is one named constant.
- `workflows` and `checklist` are shown while they have content.
- Every pill's count is its number of running entries, and a pill with none
  running shows its label alone. No pill carries a live dot: a count means
  running, no count means nothing is.
- The viewer for `shells` and `subagents` lists running entries first and puts
  finished ones under a collapsed "Completed (n)" fold. The fold's open state
  belongs to the reader and survives repaints.
- Hiding is driven by a timer set for the next expiry, not by polling.

### One TUI toggle, no Chat chip

- The Chat / TUI pair in the conversation header becomes a single pressed-state
  TUI button, shown only for agents that have a terminal. Pressed means the
  panel shows the PTY; pressing again returns to the conversation.

### Conversation menu with surface overlays

- The conversation header gains a menu (⋯) listing every surface kind that
  currently has content, with its count. Choosing one opens that surface as a
  modal overlay: the same viewer, same rows, same row actions, painted from the
  same snapshot on every refresh while open, with no height cap.
- The overlay reuses the modal scrim and dialog the confirm dialog uses, and
  dismisses on Escape or scrim click. One modal primitive serves both.
- The menu is hidden when there is nothing to list.

### Startup events live in the status line, not the chat

- `session_started` and `run_started` events are no longer painted in the
  timeline. The bridge keeps minting them; they carry the session lineage.
- While the agent shows no Working clock and the newest item in the loaded
  conversation is one of those events, the status line shows that event's
  title and age in the slot the Working ticker uses. The first Working tick
  replaces it.

## Revision 2026-09-02b: motion, and one status row

Locked after the first revision shipped. SPA only.

### One status row

- The pinned line above the composer is one row: the timer (or, before the
  first tick, the startup line) on the left, the surface pills after it, the
  git facts pinned to the right.
- Pills scroll horizontally in the space between the timer and the git facts,
  with no visible scrollbar, and fade out under the git facts on the right.
- While any pill is shown the word "Working" collapses to nothing, leaving the
  timer alone; when the last pill leaves it grows back.
- No status dot of any kind. The working colour on the timer is the only
  working signal.
- The viewer a pill opens sits above the line that tops the composer block,
  at the bottom of the conversation column, pushing the conversation up as it
  reveals. Nothing is drawn between the conversation and the viewer: no
  border, no divider.

### Motion

- One motion primitive owns every enter and exit: `reveal(element)` and
  `hide(element)` animate width and opacity (a pill grows from nothing; a
  count cap grows and shrinks at the end of its pill; "Working" collapses),
  and a taller element (the viewer, the overlay, a menu) animates height and
  opacity. Each takes about 180ms with an ease-out curve.
- Enters and exits run through one queue: several elements arriving or
  leaving in the same paint animate one after another, each starting a beat
  after the previous, never all at once. An exit finishes before the element
  is removed from the document.
- `prefers-reduced-motion: reduce` turns every animation into an immediate
  change through the same primitive; callers never branch on it.
- The keyed reconciler gains enter and exit hooks so a row that arrives or
  leaves a list goes through the primitive, and the same hooks serve the
  pill row and the viewer lists.

### Pills

- A pill's count is a filled cap at the pill's right end with its own
  background; when the count leaves, the cap shrinks away and the pill closes
  up behind it; when it arrives, the cap grows into place.

## Revision 2026-09-02c: row tickers, no Ask

- **No row actions.** The Ask menu on rows and on the workflow head is removed,
  along with the canned messages. The reader asks in the chat. The ⋯ menu in
  the conversation header and its overlays stay.
- **Row tickers.** A running entry shows a live elapsed clock in the same
  `M:SS` / `H:MM` form as the status clock, ticking once a second from its
  `started_at`; a finished entry shows its `duration_ms` as before. The row
  clock is grey, not the working colour, with the same shimmer in grey while
  it runs, and holds its width with tabular figures.
- **Shells carry `started_at`.** The bridge records the launch time on the
  shell entry so its row can tick. Fixture and wire shape updated together.

## Revision 2026-09-02d: the workflow viewer for a narrow column

The two-column workflow viewer (phases left, agents right) assumed width the
rail never has. It becomes a vertical stack.

- **Head**: one line with the workflow's name and state; the description
  beneath it, clipped to two lines.
- **Phases are collapsible sections**, stacked in order. A phase header is
  one row: title, done/total count, and an aggregate clock at the right. The
  clock is the phase's wall-clock span from its earliest agent's `started_at`:
  ticking while any agent in it runs, frozen at its last agent's end once
  none does, empty before it starts. It is styled like the row clocks (grey,
  tabular, shimmer while ticking).
- **Default open state**: the running phase is open; finished and pending
  phases start collapsed. The reader's toggles are their own and survive
  repaints (the fold is a `details` element the painter never rewrites).
- **Agent rows inside a phase are two lines.** Line one: state mark, label,
  and the row clock at the right. Line two: the model's display name (the
  catalog's label for the id, "Opus 5 · 1m" for `claude-opus-5[1m]`, falling
  back to the raw id), then the last tool or the result, clipped to one line.
  Tokens and calls leave the rail's rows; the overlay, which has width, keeps
  them on a third line.
- **Primitives.** One phase-section renderer and one phase-clock model
  function serve both the rail viewer and the overlay; the row renderer stays
  the single agent-row renderer, with a `compact` flag deciding the third
  line. Phase clocks tick through the same interval the row clocks use. No
  new painter: the phase list is one keyed list of `details` elements, each
  holding one keyed list of agent rows.

## Revision 2026-09-02e: surfaces from the local cache

- The rail seeds an agent's surfaces from the local cache the way it seeds
  the conversation: a `surfaces` record per (device, entity, agent) holding
  the last snapshot the rail saw, read on mount and on agent switch, painted
  at once (pills and, when a kind is remembered open, its viewer), then
  replaced by the first live payload.
- The record is written whenever the rail absorbs a payload whose snapshot
  differs from the one it holds, and by the background cache sync from the
  same detail payload it already reads the conversation from. It is evicted
  with the entity like every other kind.
- Nothing about visibility, grace, or the open-kind memory changes; they see
  a seeded snapshot exactly as they would a live one.
