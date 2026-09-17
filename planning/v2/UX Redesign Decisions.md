# UX Redesign Decisions

Source of truth for the 2026-08 UX redesign, implementing the five Do issues
(Build project → Planning): UX Architecture: Inbox and Control Plane,
Phase 1: Conversation Threads, Capture and Router Agent, Phase 2: Review
Prioritization, Phase 3: Implementation Graphs.

Standing rule from the reviewer: the existing implementation is NOT the
default. Where an issue doc and current code disagree, the issue doc wins and
the code changes. The theme is retained; everything else is up for rebuild.
This doc resolves the questions the issue docs leave open. Decisions here are
final for this build unless the reviewer overrides them in the thread.

## Entity model

- **Branch and issue are the only work items.** Worktrees and agents are
  implementation details, reachable only through their branch or issue.
  - 2026-09-14: workspaces — a durable multi-source checkout per project — are
    what the rail lists now, and they are per device exactly as projects are.
    A workspace's identity across the account is the pair (device, workspace
    id): each bridge mints its own workspace ids and the client never assumes
    they are unique, so one key is minted for both halves in
    `spa/src/core/deviceKey.js` and the URL names the machine
    (`#/device/<device>/project/<project>/workspace/<workspace>/…`). A
    workspace link with no device in it is resolved across the machines that
    can answer and rewrites itself.
- **Branch identity.** A branch work item is identified by
  `(project_id, branch_name)`. The bridge presents branch rows on the feed
  (`board.list`) with `kind: "branch"`, folding what are today runs, adopted
  worktrees, external worktrees, and the primary checkout into one row shape.
  The run record remains the underlying execution/lifecycle store; the wire
  and the URLs speak branch. URL: `#/project/<id>/branch/<name>/<tab>`.
- **The primary checkout is the `main` branch row.** "Main-branch agents have
  no worktree" means: their working directory is the primary checkout, and no
  worktree is created for them. The adopted-super-worktree presentation is
  replaced by the branch row.
- **Issues** are project-level work items (formerly plans). Issue agents run
  on the primary checkout. Plan documents no longer pass through a disposable
  `plan/<slug>` worktree: the planning agent writes stage docs into a
  bridge-provided scratch docs dir (outside the repo, path given in the
  prompt), and `done(phase=plan)` ingests from there straight into the store.
  The plan-worktree machinery is removed.
- **Issue implementation stays a handoff**: a new agent, new `build/<slug>`
  worktree, stage docs materialized as `base_sha`. Unchanged.

## Agents and conversations (Phase 1)

- **Agent is a first-class entity.** `AgentId` (stable ULID), with provider,
  model/effort, display ordinal, created_at, lifecycle (live/idle/ended).
  A branch can carry N agents; an issue carries exactly one agent session.
- **One conversation per agent.** Threads re-key from `thread:<entity>` to
  `thread:<agent_id>`. Existing entity threads migrate to the entity's first
  agent at boot (store migration, same pattern as prior boot migrations).
- **Multiple agents share the branch's one worktree.** No write lock between
  them — enforcement by observation (design rule 2): the Changes view makes
  what happened legible; the human decides. The run-level single-active-writer
  rule remains only for issue implementations.
- **PTY keying** moves from `(worktree_root, AGENT_TAB_ID)` to
  `(worktree_root, agent_id)`. `agent.add` creates a new agent on a branch;
  `term.create` still refuses harness kinds (agents are created only via
  `agent.add`, so they are always Build-owned).
- **Events carry a class: `attention` or `status`.** Attention: agent
  messages, questions, implementation done, new user-visible comments,
  blocked. Status: run started, commits, revisions, working time, stage
  progress. The class lives on `ThreadEventKind` (single source of truth in
  the bridge).
- **Unread is event-driven.** Each entity keeps a per-user read cursor
  (`last_read_sequence`) in the attention store; `entity.seen` advances it.
  An entry is unread iff an attention-class item exists past the cursor.
  State-derived `needs_attention` remains only as an input that emits
  attention events (e.g. entering `plan_review` emits one); it no longer
  drives unread directly. Unread entries carry the reason (the latest
  attention event kind) for "why it needs you".
- **Per-agent unread counts** ship on the wire for bubble badges; the entity
  row's unread is the union of its agents' unread plus entity-level attention
  events.
- **Mute** is a per-entry flag in the attention store (`entity.mute`). Muted:
  no push, no unread badge, entry stays in the inbox with live status.
- **Completion reports are requested.** `done`'s input schema gains
  `completion_report` (critical_files, risk_notes, decisions, skips); build /
  revise phase templates ask for it. The two assertions that enforce its
  omission are removed. The report renders as a card in the conversation.
- **MCP history query tool** `search_conversation`: text + filters (file,
  commit, stage, role, since). Items get derived findability metadata at
  post/event time: commits referenced, files touched, stage links.
- **Related-conversation updates**: a run's attention-class outcomes (done,
  blocked, failed, merged, abandoned) mirror onto the parent issue's
  conversation as events; status-class events do not.
- **Done event needs no agent comment**: `post_completion` stops posting a
  companion agent message; the event is the record.
- **Plan-doc comments become conversation posts** on the same anchored-message
  path as diff comments (`MessageAnchor` gains `artifact: "doc"`). The
  separate `StageComment` model is retired.
- **Working time and diffstat leave the conversation timeline** and render as
  toolbar status. Authoritative source: seen-at-derived turn timing (the rule
  the MCP tool descriptions already teach agents).

## Shell and inbox (UX Architecture)

- **Three-panel universal layout**: inbox (left, global, persistent) —
  view area (center) — agent rail (right, owned by the selected branch/issue,
  below the toolbar). One layout for every non-modal view.
- **Inbox** is one list across all projects (device-scoped — it describes the
  connected bridge; that is what "global" can honestly mean in this topology).
  Entries are branches and issues with three states: unread (attention event
  pending, shows why), working (agent read the message, not yet done),
  inactive. Status events update metadata silently.
  - 2026-09-14: account-wide as of the multi-device stages. The client holds a
    session per paired device and merges their feeds, so the inbox and the
    projects rail list every machine's work at once, each row naming the
    machine it is on. The device picker filters that list ("All devices" or
    one machine) and nothing else — it does not move where anything runs.
- **Done button** on an entry when its branch is committed+pushed or its
  issue is marked implemented. Done archives the entry. For issue
  implementations it marks worktree + issue together, with a disclosure
  control to override the linked marking. Merge and other lifecycle verbs
  live in the Changes actionbar, not the inbox.
- **Dedup**: once implementation starts, the issue's work surfaces as the
  branch entry only.
- **Account access at the inbox bottom** (settings, devices, archive live
  behind it as account pages). Notifications-as-landing is replaced by the
  inbox; deep links canonicalize.
- **Toolbar**: project selector + branch/issue selector as parallel entries
  of one project-scoped menu, which can also create either (creating opens
  the item; the agent session starts on the first message — issues too: no
  planning agent is spawned until the first message or an implement action).
  Working time and +/− counts pin above the agent rail's composer instead of
  the toolbar, alongside ahead/behind when nonzero.
- **Branch tabs: Changes, Files. Nothing else.** Conversation and Agent tabs
  are deleted; both live in the agent rail. Terminals move to the console.
- **Changes**: left column = `Uncommitted` (with +/− counts) as top entry,
  the review aggregate ("All changes") directly under it when the surface has
  one, commit list under both; no pinned "All changes" default selection —
  the row exists to be reached without scrolling, not to open on load.
  Review-plug content (triage,
  comments) renders on the Uncommitted/selected changeset. Commit box
  discloses only while uncommitted changes exist. Viewer = stacked full file
  diffs, line numbers, per-file header with counts and a Comment button, in
  every changeset (commits included). The separate changed-files/staging list
  is removed — commit is commit-all; per-file discard moves to the file
  header's ⋯. Noise files render as a collapsed group, never filtered out.
- **Issue view**: persistent two-column stages | stage viewer. No tabs. The
  stage column carries the worktree/agent assignment control (new or existing
  worktree, new or existing agent).
- **Console**: collapsible bottom panel hosting the terminal tabs for the
  selected branch/issue's checkout (primary checkout for issues and main).
  Collapsed by default; half and full overlay sizes.
- **Agent rail**: right-edge bubble strip, always present — one bubble per
  agent (unread badge, working spinner, active highlight), `+` add-agent on
  branches only. Tapping expands that agent's conversation panel. Panel
  header: agent identity, Chat/TUI toggle, collapse. TUI mode attaches the
  same panel to the agent's PTY (`term.resize` to panel geometry).
- **Routing**: `#/inbox` is the landing route. Branch/issue routes lose the
  conversation/agent tabs; legacy URLs alias to the nearest surface.

## Capture and router

- **Global compose**, inbox-adjacent and available on every route (including
  the gate, queued until a device connects). Keyboard shortcut `c`.
  Submitting hands text to the router; no routing decisions at capture time.
  The FAB's create actions fold into the toolbar menu and compose; the FAB is
  removed.
- **Captures are durable before routing**: a `captures/` store record is
  written first; offline captures queue client-side and flush on reconnect.
- **Router = device-scoped v1** (the honest scope; account-wide fan-in is a
  follow-on once multi-device aggregation exists). It runs on the connected
  bridge as a new session-owner kind (`router`), spawned reactively per
  capture in a bridge-owned scratch cwd, never a repo checkout.
  - 2026-09-14: the reading side is account-wide now, the capture side is not.
    A capture goes to one machine — the **Creation device** on account Settings
    ("New projects and captures go to"), which is also where new projects are
    made — and that machine's router answers it. Account-wide fan-in is still
    a follow-on.
- **Router MCP surface** (separate server identity; coding agents never see
  these tools): `list_projects`, `list_work`, `read_conversation`,
  `create_issue` (inert), `dispatch_branch`, `ask_user`. Write scope is
  creating issues and dispatching branch work; nothing else.
  - 2026-09-17: `create_issue` is gone. Router issue destinations were retired
    with Issues, so the surface is the reads plus `dispatch_branch`, `ask_user`
    and `post_thread_message`, and the allow-list a harness is given says so.
- **Project MCP surface** (2026-09-17; same server, resolved from a `project-`
  agent id as the router's is from `router-`): `list_workspaces`,
  `list_workspace_agents`, plus the conversation tools. Read-only, scoped to the
  project its owner is bound to. See `workspaces.md`, "The project agent".
  - 2026-09-17: the writes landed beside the reads — `create_workspace`,
    `add_workspace_agent`, `remove_workspace_agent`, `message_workspace_agent`,
    each a thin wrapper over the verb the client calls, each scoped by the same
    owner binding rather than by an argument. Deleting a workspace and adding a
    directory to one are deferred by policy: both are how a project loses work.
- **Inert issues**: `issue.create` gains `dispatch: false` (router default) —
  a record in the store, no worktree, no agent, until the user opens it and
  sends a message or triggers implement.
- **One-call dispatch**: `branch.dispatch {project_id, branch?, instruction}`
  performs worktree-create/adopt + agent create + first message atomically,
  cleaning up on partial failure.
- **Decision rule** (testable): dispatch to a branch only when the capture
  names an existing branch/worktree or unambiguously continues work already
  in flight there; otherwise create an inert issue on the best-guess project.
  Clarifying question (`ask_user`) only when even the project is ambiguous —
  it surfaces as the capture entry's unread reason, answered from the inbox.
- **Routing result is an inbox entry**: captured → routed to <project> as
  <issue|branch>, unread, with one-tap reroute (moves the capture to a new
  destination via the same create/dispatch verbs; the misroute artifact is
  archived if untouched, kept and linked if work already happened).
- **Router model**: account default provider at low effort; overridable in
  settings.

## Review prioritization (Phase 2)

- **Granularity**: hunk-level. `diff.js` assigns stable hunk ids
  (`path @@ index + content hash`); triage classifies hunks, groups render
  per-file and per-group.
- **Levels**: `critical | normal | low`. Critical files/hunks surface first
  (ordering), low collapses into named groups with one-line rationales;
  normal renders as today. Everything stays in the DOM.
- **Execution**: a `triage` phase turn on the run's worktree agent after
  done(build|revise), modeled on validate: TRIAGE template, typed
  `done(phase=triage, outputs.triage)` with per-hunk
  `{hunk_id, level, rationale, group}`. Seeded by the completion report.
- **Persistence**: triage result + `based_on` revision sha on the run record;
  stale triage renders with a "stale — re-triaging" chip when the diff moved.
  Re-runs on each new revision.
- **Overrides**: expanding a collapsed group/hunk or collapsing a surfaced
  one offers "disagree" → `triage.override {hunk_id, direction, note?}`,
  persisted with the triage result and posted to the agent's conversation as
  a status event. Overrides are durable per-project signal
  (`.build/review-rules.json` seed for the later learned-defaults layer).
- **Trust dial**: a per-view toggle renders the untriaged full stack.
- **No triage → plain full stack**, explicitly labeled, never an empty diff.
- Triage is presentational only; it gates nothing.

## Implementation graphs (Phase 3)

- **The graph wraps stages**: `stages.json` becomes the degenerate linear
  graph. Canonical artifact `graph.json` in the issue's store docs (nodes:
  plan/implement/validate/review/prompt; edges with optional conditions;
  bounded loops, max-iteration required). No parallel fan-out v1 — one
  worktree per implementation.
- **Owned by the issue** after approval; dev edits in the SPA write to the
  store; edits mid-run apply from the next node boundary.
- **The orchestrator agent is the issue's one agent session** (satisfies
  "exactly one agent session per issue"), running on the primary checkout,
  with orchestrator MCP tools: `graph_state`, `dispatch_node`, `skip_node`,
  `reroute`, `pause_graph`, `resume_graph`. It never implements a node
  itself — handoff always wins on that conflict.
- **Node failure**: orchestrator decides pause / reroute / retry within loop
  bounds; the decision posts as an attention event. Reroute-then-report, not
  ask-first (design rule 2), except graph pause which is an attention event
  awaiting the user.
- **Review nodes** consume the Phase 2 triage artifact (level counts +
  critical hunks) and can hold the graph until human approval.
- **Per-node model/effort config**, defaulting to the issue's choice; a node
  naming a different provider forces a cold turn (accepted cost).
- **Execution-mode choice per issue**: direct-to-agent (one-click), stages
  (linear graph), or graph-with-help; chosen at implement time, not at
  capture.

## Cross-cutting

- **Push/badge**: pushes fire on attention events (debounced per entry, 60s),
  not only state changes; `attention` kind covers new event types — no new
  ALLOWED_KINDS needed beyond what exists.
- **Terminology on the wire**: new verbs speak `branch.*` / `agent.*` /
  `capture.*` / `triage.*` / `graph.*`. Existing `run.*`/`worktree.*` verbs
  keep working during the transition and are removed once the SPA no longer
  calls them.
- **Migrations**: store migrations at boot (threads→agent-keyed, attention
  cursor backfill); no data loss; `plan/<slug>` worktrees get cleaned up by
  the existing disposal path.
- **Testing bar**: TDD throughout — bridge `cargo test` (+ clippy -D
  warnings, fmt), SPA vitest for pure logic, `spa:verify` browser pass per
  SPA milestone. semgrep + gitleaks before every commit.
- **Delivery order**: A) bridge foundations (events/attention/agents/threads/
  mute/completion/query tools) → B) SPA shell (three-panel, inbox, toolbar,
  rail, Changes/Files, issue view, console) → C) capture + router →
  D) triage → E) graphs. Do issues map: Phase 1 validates after A+B rail;
  UX Architecture after B; Capture after C; Phase 2 after D; Phase 3 after E.
