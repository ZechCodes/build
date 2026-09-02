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
- **Recorded gap.** No captured run in `bridge/tests/fixtures/claude-stream/`
  carries a `TodoWrite` call, so the wholesale-replace path is pinned against a
  hand-written call block rather than a fixture. Its field names
  (`todos[].content`, `todos[].status`) are unverified against a real stream.
  Capture one and drive the replace test from it when a session using
  `TodoWrite` can be recorded.

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
