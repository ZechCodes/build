# Project workspace

Planning amendment for **#522**, 2026-10-12. **Proposed, not implemented.**
Code audit baseline: `d4bd92fd2ed2cbfc8b0ae02648e6e8d601d7f1c4`.
Implementation is on separately reviewed child branches, after the decisions
below. Opus implements; the #522 planner reviews every child. Filed children are
#529–#538, linked to #522 and left unassigned in Backlog. This document
supersedes the older planning documents only for the scope it describes.
`ARCHITECTURE.md` and the code still describe what ships today.

## Decisions for Zech

1. **Dirty or locally committed base:** recommend retaining #267's `sync_base`
   setting and defaults. Fetch and safe fast-forward remain automatic; dirty,
   ahead, diverged or in-progress bases are skipped with a visible reason.
   Never silently stash, reset, rebase, discard, or disable the setting. Manual
   **Sync base** has the same safety rules. Build-managed Git mutations share
   the sync lock; a user's terminal or an agent's shell is not controlled by it.
2. **Worker commits:** recommend allowing direct commits on the source's current
   branch, including the configured base. This is the requested shared working
   place. Branch-first in the same checkout would change every worker's branch
   and would not isolate them. For independent work, create an isolated workspace.
   Neither the prompt nor a Git guard is an OS sandbox. The project Changes face
   is the observable record; it cannot attribute a shared working-tree edit to
   an individual agent with certainty.
3. **Everyday names:** recommend **Project workspace** for the original-source
   destination in a navigation menu and explanatory text; keep the project name
   on its bar/inbox head. Named checkout rows and bars use their given names;
   creation says **New workspace**, collection says **Workspaces**. “Isolated
   workspace” is a spec distinction, not a mandatory prefix in every label.
4. **Terminal placement:** recommend the existing console with a **Terminal**
   rail action (a toggle outside the face tablist), not a new full-page face.
   It is available on every project face and project-hosted task, never opens
   or starts a process by default. Source choice applies to new tabs only.
5. **Source movement with live processes:** recommend refusing source move/remove
   while a source shell or project worker session is live, naming what must be
   closed. Do not kill a user shell as a side effect or retarget it to a new
   folder. Project deletion is the explicit operation that retires the whole
   project's sessions while preserving its original sources.

The project agent has agreed the owner design: reuse the project roster,
exactly one persisted pinned coordinator, source-writing workers below it,
coding/workspace tools for workers, coordinator orchestration tools unchanged,
bridge protection of the pin, and project deletion as the whole-project exit.
These are settled implementation constraints, separate from Zech's decisions.

## Vocabulary, page shape and navigation boundary

**Workspace** names the rail-based view with Tasks, Changes, Files, attached
agents and a terminal. **Project workspace** stands on the original configured
sources and adds the Workspaces face. **Isolated workspace** is a named checkout
cut from those sources; its existing persisted `workspace_id` remains unchanged.
**Project** remains the containing entity and settings boundary. A **branch
page** remains a checkout view (Changes/Files); it does not acquire a workspace
record, Tasks face or newly created agents through this terminology change.
A **task** is a detail page within its host's Tasks face.

- Project faces: Tasks (existing dashboard default), Changes, Files, Workspaces.
  Keep project-wide tasks; do not narrow them to its new workers. Named workspace
  Tasks retains its existing agent-assignment filter and list default.
- One source tab per configured source in Changes; use the shared Git pane.
  Files retains its shared multi-root explorer. Changing faces keeps the agent
  rail and console, plus per-source drafts, selected commit and scroll position.
- The Workspaces face lists **named children only**, including lifecycle states,
  size, unread/activity, filters and existing management actions. Do not insert
  a self row, invent a workspace ID for the project or offer Reclaim on its base.
- Extend `projectRailState.js` / `projectReturnRoute` to remember Changes and its
  source; canonical project links still open Tasks and explicit deep links win.
  #523 uses this return helper for Up without inferring context from assignment.
- #523 owns the breadcrumb, sibling selector, global switcher and cross-project
  navigation. Both plans use the same cached roster, device/project/workspace
  keys, display-name helper and open-target factory. The Workspaces face owns
  management/size/reclaim controls; the sibling list is a navigation projection
  and does not duplicate them. Filter/order can differ for their purposes.
- Keep #523's destinations valid before and after this change; its project
  destination already exists. Its navigation work need not wait for this Git API.
- `/project/p/tasks/t` remains `name: trackerTask`, hosted by the project.
  `/project/p/workspace/w/tasks/t` remains `name: workspace` plus `taskId`, hosted
  by that named workspace. Never infer the host from assignment, last visit or
  agent ID. Project-hosted tasks share the project roster/console; a task inside
  a named workspace keeps only the pin and that workspace's agents.

### Complete vocabulary and route change inventory for this feature

| Surface or contract | Planned text/change | Kept deliberately |
| --- | --- | --- |
| Project bar and inbox project head | Project name; explanatory/accessibility context may say “Project workspace” | Entity remains Project; #523 owns breadcrumb controls |
| Navigation original-source destination (#523) | “Project workspace” | Not a child workspace or duplicated self row |
| Named workspace bar and inbox/list row | User's workspace name | “Workspace” fallback; no “isolated” prefix |
| Creation sheet/action | “New workspace”; help may explain “Creates a separate checkout of the project sources” | Existing creation/isolation controls and IDs |
| Rail labels/tooltips/accessible names | Add “Changes” and “Terminal” on project workspace | “Tasks”, “Files”, “Workspaces”, “Project settings”, “Workspace settings” |
| Project agent rail | “Project agent” for pin; given worker names; “Add agent”; empty owned list explains source work | Pin remains reachable elsewhere; Projects inbox face remains “Projects” |
| Source Git actions | “Commit”, “Fetch”, “Pull”, “Push”, “Stash”, “Apply stash”, branch selector; “Sync base” explicitly names configured base | Do not relabel Pull as Sync base |
| Source context | “Original sources · shared checkout”; current branch and configured base when different | No isolation claim |
| Plain source | “This source is not a Git repository”; Files and Terminal remain usable | No automatic Git initialization |
| Old bridge | “Update this device to use project Changes/Terminal/agents” for unavailable feature, based on cached capability | Existing Files fallback and pinned chat remain usable |
| Route names | **None renamed:** `project`, `workspace`, `branch`, `trackerTask`, legacy `task` | Existing IDs, resolve aliases and device-qualified URLs |
| Project Changes URL | Add `/project/p/changes?source=s&commit=sha` (device prefix when known) | `/project/p` remains Tasks; Files `source`, `file`, `line` queries survive |
| Agent selection URL | Existing `agent` query admits workers only on project/project-hosted task routes | Foreign local-agent link must not select a different agent |
| Terminal URL/state | Existing console UI state; no `/terminal` page | Existing legacy terminal aliases retain their compatibility behavior |
| This spec heading | New “Project workspace” | Entity/schema names not renamed |
| UI Design Brief amendment | New “Workspace views”; supersedes original Information Architecture/Task view/Terminal descriptions in this scope | Historical journeys are not wholesale rewritten |
| Scope amendment | New “Project workspace scope”; replaces worktree-per-task/no-agent-touches-main assumptions for workers | Historical lifecycle text remains historical |
| Changes View amendment | New “Project source Changes” | Shared keyed diff and editor state requirements |
| Agent Surfaces amendment | New “Pinned coordinator and project workers” | Harness subagents remain distinct from Build roster agents |
| Architecture at implementation | Amend “Workspaces and worktrees”, “The SPA / Surfaces”, “MCP tools”, “Wire versioning and capabilities” in owning child | Headings/anchors and unrelated docs remain stable |

No broad search-and-replace of “project” or “workspace”. #523 owns wording in
its new navigation controls; the table is its vocabulary contract. C9 checks
visible strings, accessible names, prompts/help and doc references together.

## Findings that correct the brief and stale comments

The brief's suggestion that a **new owner model** is necessary is wrong.
`AgentOwner::project` and a multi-agent project conversation roster already
exist. `agent.add` can add project agents; the SPA hides Add. Every such agent
currently gets a `project-*` ID and coordinator-like semantics. The missing
concept is **purpose within that roster**, not another owner kind.

Other implementation facts:

- `git.* {project_id}` already reaches the first source; `{project_id, source_id}`
  is not supported (`app/git/mod.rs`, `resolve_workspace_git_scope`).
- `term.create/list {project_id}` already uses the first source. The legacy
  parser ignores `source_id`; `term.attach` checks supplied ownership only for
  workspace scope. Copying Files params into those calls would be incorrect.
- `fs.projectSources` and `fs.projectSourcePath` already provide explicit source
  membership and stale-location checks. They do not announce Git or terminal
  support. Share the boundary, add separate capabilities.
- `app/facts.rs::git_subjects` covers Git home, runs and external worktrees;
  `worktree_roots`/`note_terminals_at` derive from that. Secondary sources and
  non-Git home need new source-qualified watcher/subscription coverage.
- `agent.remove` currently permits the primary/last project agent. Pin protection
  must be in the bridge, not just a missing menu item.
- Sources can be unrelated directories; there is no common project container.
  Current worker-capable processes start in home; `.build` scratch and process
  cwd are separate concepts. The coordinator prompt explicitly forbids edits.
- The Git pane is commit-all (`gitPane.js` stages all dirty paths), although
  `git.commit` itself commits the index. There is no existing unstage/hunk action.
- #267 already leaves dirty/local-commit bases alone. #271 can cut from a fetched
  fast-forward target before the checked-out base finishes advancing; it is
  incorrect to promise every cut uses the source checkout's current HEAD.
- The existing toolbar's project crumb is not clickable, **but its menu does
  offer the project's page**. “No way up” describes the missing direct gesture,
  not absence of a destination. #523 fixes that interaction.

Line references below are baseline coordinates, not future line numbers:

| File:line | Disagreement or comment requiring amendment | Owning change |
| --- | --- | --- |
| `spa/src/core/router.js:97` | Says project main pane is its workspaces; Tasks is already the default | C6 / #534 |
| `spa/src/core/router.js:99` | Says source checkout is not a place to work; Files already edits it | C6 / #534 |
| `spa/src/core/router.js:196` | Says checkout has no surface of its own; Files now is such a surface | C6 / #534 |
| `spa/src/core/router.js:289` | Says both Tasks faces have two layouts; project dashboard is also supported | C6 / #534 |
| `spa/src/core/toolbarModel.js:38` | Says project has no conversation of its own; project conversation already exists | #523 navigation model child |
| `spa/src/core/toolbarRender.js:1` and `:27` | Calls one button a breadcrumb; markup is one menu trigger, not linked segments | #523 breadcrumb child |
| `spa/src/core/toolbarModel.js:1` | Accurately describes today's single picker; update alongside #523 split controls | #523 navigation model child |
| `spa/src/core/toolbarRender.js:21` | “task” here is legacy `task`, not trackerTask; qualify it during navigation rewrite | #523 breadcrumb child |

Older scope text describes relay application fallback, PTY-only agents and one
MCP tool; these are also historical, not implementation assumptions for #522.
Agent Surfaces' `entity.changed`, `branch.get` and `plan.get` references predate
wire 3.0 removals. Use current `changes` and conversation APIs; do not revive them.

## Project source Git contract

### Scope and wire

Extend the existing scope algebra with **exactly**
`{project_id, source_id, source_path}`. Resolve membership using server records,
canonicalize the configured path, and compare `source_path` as a captured-location
assertion; never use it as a caller-provided root. Reject unknown, empty,
malformed, cross-project and mixed run/workspace/worktree selectors. Require the
assertion on new explicit source operations. Keep legacy `{project_id}` primary
scope unchanged. Revalidate after waiting for off-lock work before mutation.

New named capability **`git.projectSources`** gates these added selectors on:
`git.status`, `git.diff`, `git.changeset_diff`, `git.log`, `git.show`,
`git.unpushed`, `git.refs`, `git.stage`, `git.discard`, `git.commit`, `git.fetch`,
`git.pull`, `git.push`, `git.stash`, `git.stash_pop`, `git.merge_abort`,
`git.checkout_ref`. These verbs already exist; there is no new parallel Git API.
Keep diff paging, conditional `if_status_key`/`if_diff_key` reads and existing
path/ref validation. Choose project history highlighting explicitly rather than
letting the presence of a cache namespace imply workspace review semantics.

Existing `project.sync_source` remains **Sync base**, by registered source ID.
Its request gains capability-gated `source_path` precondition under
**`sources.syncBasePath`** so an old screen cannot sync a moved source. Carry
that assertion through the asynchronous sync request queue and revalidate it
after acquiring the sync lock, not just when accepting the request.
Non-Git sources offer Files/Terminal and an accurate Changes empty state.
Do not call project-home `project.init_git` for a secondary source; source Git
initialization UI is deferred. Unborn repositories need status/stage/first commit
without a fabricated history or remote base.

### Offered operations

- Use the shared pane's existing **commit-all** action: it stages all dirty paths
  with `git.stage`, then calls `git.commit` (which commits the index). Do not
  claim selective staging, unstaging or hunk mutations exist; those UI additions
  are deferred. Commit targets actual HEAD, never silently checks out base.
- Fetch and normal Push using existing validation; no new force-push/reset action.
- Pull remains the shared pane's explicit ff/merge/rebase operation where supported;
  it is user-directed and separate from the background service's ff-only policy.
- Stash and apply/pop retain existing conflict behavior; never automatically stash
  to make branch switching or base sync pass.
- Branch switching uses `git.checkout_ref`, its dirty/operation checks and normal
  Git linked-worktree restrictions; it does **not** update configured `base_branch`.
  Remote checkout/local-branch creation follows the existing verb, with literal args.
- Discard and abort retain existing explicit destructive-action confirmation.
- Show actual current branch, configured base, upstream status and last base-sync
  outcome. Shared checkout notice appears with source context and worker creation.
  No agent approval ceremony or fictitious per-worker diff isolation.

### Coexistence with synced sources and cuts

Reuse `source_sync::SyncLock`, keyed by common Git directory, for Build-managed
source mutations, base sync, source updates and source-dependent cut preparation.
Acquire outside `AppState` lock; never wait for Git under it. The sync lock is
non-reentrant: acquire once per canonical common Git directory, deduplicate
multiple source aliases and order any multi-repository acquisitions. Pin exact
cut OIDs while protected if materialization follows after releasing the lock;
do not wrap the existing service/cut sync in a second acquisition. Revalidate source
location and Git preconditions when the operation gets its turn. Keep unattended
fetch timeouts, credential redaction, hook suppression and ff-only behavior for
base sync. User-requested Git verbs retain their existing execution policy.

| Source state | Sync base | New isolated workspace |
| --- | --- | --- |
| Clean, behind, safe fast-forward | Fetch + ff; preserve #271's bounded cut path | May start at fetched upstream commit while service advances base |
| Dirty/untracked collision/in-progress | Skip with reason; leave files/index alone | Git cut uses configured base's committed state and existing warning; does not copy uncommitted tracked edits |
| Configured base has local commits, ahead/diverged | Preserve local branch; report skip | Starts from those local base commits; disclose that they are included |
| Source HEAD on another branch | Existing safe base-ref sync rules | Starts from configured base, not that other branch's HEAD |
| `sync_base: false` | No automatic/cut sync; explicit Sync base still fetches and safely advances without enabling automatic sync | Uses configured local base |
| Plain folder | No Git sync | Existing directory-copy isolation rules |

Preserve worktree versus Rift copying rules, including their existing handling
of ignored files; do not promise all workspace bytes come only from Git commits.
The shared lock does not serialize shell Git or OS writes. Recheck, report conflicts
and retain observable status; do not describe enforcement-by-observation as safety
against simultaneous arbitrary shell edits.

## Push, cache and compatibility contract

Reuse push **`changes`**, with existing kinds `git`, `files`, `terminals`, `state`, `thread`, `tasks`
and existing project/config list invalidations. There is no `agents` change kind. Extend source-qualified identity and filters
under **`changes.projectSources`**: project ID + source ID + captured source path
on source facts/events and matching subscription selectors. Home legacy facts
remain available for old clients; publish explicit source facts only to subscribers
that ask for them, avoiding duplicate or unknown rows in old consumers.

- Add every registered source to appropriate watchers; plain folders need file
  and terminal events without a Git repository. Shell EOF/close and external Git
  edits update the correct source. Source settings changes invalidate membership.
- SPA record address is account/device + project + source + captured location +
  record kind/subkey. Status keys alone do not identify a source. Keep immutable
  commit patches reusable only within their source identity; invalidate mutable
  refs/history/status/diffs on checkout. Source move creates a new namespace.
- Pulls and pushes write cache first; surfaces paint subscribed cache records.
  A stale response or removed-source event cannot restore an old scope or move a
  draft. Preserve source drafts visibly and refuse stale writes.
- Cache support flags for cold mounts; dispatch added fields only after the current
  compatible greeting. Unknown-capability clients keep existing scope contracts.
  Reconnect rehydrates source lists/status/terminal rosters without clearing live
  DOM, drafts or selected agent. Do not introduce data polling.
- Add fixtures and adapter support alongside each bridge contract. Use the next
  available additive minor at merge time, shared by additions in the same release;
  do not reserve a stale numeric version in this plan. Update `API_VERSION`,
  versions/hello fixtures and previous-release manifest per Architecture.
- Preserve `workspace.*`, `project.*`, `tasks.*`, existing owner IDs, route names,
  persisted workspace records, `@workspace` references, transport envelope and
  protocol version. Terminology alone has **no wire rename or major bump**.

## Project terminal contract

New SPA console kind **`project`**; session context is project, source and captured
location. Bridge internal scope adds a project-source variant without a new
persisted workspace. New capability **`term.projectSources`** announces explicit
selectors on `term.create`, `term.list`, `term.attach`; extend scope validation to
`term.close` when the new caller supplies its source assertion. Preserve existing
terminal-ID input/resize/ack transport and `term.output`, `term.reset`, `term.closed`.
No extra channel, harness PTY reuse or new terminal engine.

- New tab starts the user's shell in the selected source's canonical original
  directory. Remember choice per project/device; Files selection may seed it
  initially but never moves an existing shell. Label tabs by source plus tab name.
- UI/list key: `(device, project, source, captured_location)`; shell identity is
  its unique terminal ID with immutable owning scope and canonical cwd. Preserve
  canonical-path legacy aliases without permitting a supplied foreign source to
  attach another source's terminal. Do not deduplicate merely by displayed name.
- Closing console, switching face/project, or losing a client detaches; it does
  not kill the shell. Explicit tab close, EOF, missing root and project deletion
  retire/reap using existing terminal lifecycle. Bridge restart follows existing
  terminal behavior; do not promise restart-persistent PTYs.
- Keep existing 16-shell daemon cap and `ChildKind::Terminal` placement.
  Agent sessions remain separate. Plain sources work just like Git sources.
- C3 adds source move/remove holds for live shells. C5 adds project-worker holds;
  because a worker may use all configured sources, any live worker holds all
  source membership/path moves. This is lifecycle consistency, not filesystem
  isolation. Holds include in-flight shell creation and worker spawn/delivery
  reservations, atomically checked against move/remove admission and retained
  across deferred source-update settlement. Once processes are closed, old scope IDs cannot attach replacement
  paths. Renaming a label alone need not end anything.

## Pinned coordinator and project workers

### Persistence and API

Reuse the scratch conversation owner created by `project.ensure_conversation`
and its existing `AgentRoster`. Persist **`pinned_agent_id`** on that owner and
an agent purpose **`project_role: coordinator | worker`** (distinct from model
selection's existing planner/implementer/reviewer `role`). The roster is the owner;
each agent still has its own canonical conversation, settings, draft and session.

- Ensure-conversation idempotently ensures one coordinator, never one per call.
  Existing primary becomes the pin in migration; existing extra project agents
  become workers while preserving agent IDs, canonical history and settings.
  Empty legacy roster creates a single pin. Test migration twice. Never select a
  replacement pin by current array position after deletion or refresh.
- Preserve existing `project-*` IDs. A prefix is no longer enough to select
  prompt/MCP surface; resolve stored purpose from exact owner + agent. New workers
  may retain the existing project ID minting path. Never create fake workspace IDs.
- Capability **`agents.projectWorkers`** announces purpose/pin fields on project
  rosters and worker creation. Reuse `agent.add {entity_id,...}` for explicit
  project-worker creation with capability-gated `project_role: worker`; the bridge
  alone creates the coordinator. `agent.list`, choose/name/watch, thread APIs,
  reset and removal retain exact agent/conversation addressing.
- Guard `agent.remove` and any owner-rebinding/removal paths in the bridge: no
  surface may retire, transfer, demote or replace the pin while the project exists.
  Ordinary task assignment **to** it remains possible; the prohibition concerns
  changing its owner/purpose, not assigning it work. Clear conversation replaces
  history/session while retaining pinned identity. `project.delete` alone tears
  down the entire project lifecycle and preserves original source files.
- Old clients get the compatible pinned-only project projection unless they
  announce worker support; they must not paint every worker as a second pin.
  C4 adds capability-gated `include_project_workers: true` to `agent.list`. Legacy project summaries keep the
  pinned-only projection; existing `state`/`thread` invalidations make new clients
  refresh that separate complete roster cache with the gated list parameter.
  Do not add an ungated hello parameter or filter canonical persistence.

### Work, tools and process roots

A worker starts at project home, is told every source label/path, and can read,
edit and run commands in the original sources. It shares their working trees,
indexes and checked-out branches with the human and other workers. It may `cd`
to another source; there is no symlinked artificial root or claim of sandboxing.
The pin retains its present read-only-by-instruction policy and delegates edits.

Workers receive **`McpSurface::Coding`**, including the ordinary conversation,
task and workspace tools. They can create a workspace and delegate independent
work, message same-project agents, report progress and use task reviews. They do
not inherit coordinator-only source add/remove and project orchestration tools.
Add a worker prompt that explains source writes, shared Git and explicit progress;
do not deliver a phased-run prompt that invents a plan/build worktree lifecycle.

MCP launch and daemon authorization must use persisted purpose, not merely a
`project-` ID or a client-provided surface flag. A copied flag must not grant
coordinator tools. Existing per-session capability, generation and exact-address
checks still apply. Add **`list_project_agents`** and **`add_project_agent`** thin
MCP wrappers on project and coding surfaces, implicitly scoped to the caller's
project; `message_agent` already provides messaging. Creation remains unwatched
unless the human opted in, with existing `created_by` lineage stamping.

Use the existing project conversation's private scratch run for scaffolding,
with per-agent session/MCP/compaction artifacts keyed by exact agent/session.
Keep those artifacts out of source roots, and don't use shared cwd as resume
identity. Reuse existing `agent_process_cwd` and harness launch abstractions;
worker startup fails visibly if its captured source configuration has moved.
Shared cwd makes provider cleanup especially sensitive: reset/remove one worker
must never delete sibling or coordinator history merely because they share cwd.

### Assignment, visibility and deletion

- `assign_task {kind: agent, agent_id}` already addresses same-project agents;
  include project workers in the SPA assignee roster and immutable task links.
- Add **`kind: new_project_agent`** to `tasks.assign`/MCP `assign_task`, behind
  **`tasks.assignProjectAgent`**, with name and ordinary role/model/effort choices.
  Project is the task/caller's validated project; do not overload `new_agent`,
  whose `workspace_id` remains required. User, workspace, agent, new_workspace
  and existing new_agent assignment semantics remain unchanged.
- The project workspace rail renders its one pin above the line and workers
  below, named by the user just like workspace agents; Add creates a worker.
  Project-hosted task pages keep this rail. Isolated workspace/branch rails show
  only the pin plus their own agents; a worker is never pinned elsewhere.
  The pin's conversation is rendered once, not duplicated in both halves.
- Rail selection/drafts/unread counts/lineage distinguish exact agent IDs. Tasks
  and inbox summaries must count each watched conversation once. A worker may
  appear as the assignee of a task elsewhere without becoming a rail resident.
- Worker remove stops/reaps only its session, revokes authority and pending sends,
  handles task assignment/tracking consistently with existing agent removal, and
  preserves sibling drafts/history. No source files or branches are deleted.
- Automatic workspace reclaim never sees the project owner as a disposable
  workspace. Deleting/reclaiming a named child cannot retire project workers.
  Whole-project deletion retires workers, pin and source shells, cleans only
  owned scratch/session artifacts, and preserves source directories.

## Sequenced child tasks

All children are **backlog until Zech settles the relevant decisions**. One task,
one branch from main, one review. Dependencies mean merged/reviewed contracts,
not unreviewed stacked implementation. Size is engineering half-days, including
focused tests and review fixes; estimates are not calendar commitments.

| Child | Scope | Depends on | Half-days |
| --- | --- | --- | ---: |
| C1 / #529 | Bridge: original-source Git scope, source facts and subscriptions | Decision 1 | 3 |
| C2 / #530 | Bridge: serialize source Git/base sync and pin workspace-cut behavior | C1, decisions 1–2 | 2 |
| C3 / #531 | Bridge: original-source terminal scope and lifecycle | C1, decisions 4–5 | 2 |
| C4 / #532 | Bridge: pinned coordinator and project-worker persistence/runtime | Agreed role model | 4 |
| C5 / #533 | Bridge: project-worker tools, assignment and cleanup | C4, decision 5 | 3 |
| C6 / #534 | SPA: project Changes and workspace page shape | C1, C2 | 3 |
| C7 / #535 | SPA: project console and source tabs | C3, C6 | 2 |
| C8 / #536 | SPA: project worker rail, assignees and cached roster | C4, C5, C6 | 3 |
| C9 / #537 | Workspace terminology and source/comment documentation | C6–C8, decision 3; coordinate #523 | 1 |
| C10 / #538 | Combined compatibility/lifecycle/browser acceptance | C2–C9 | 2 |

**Total: 25 half-days (12.5 engineering days).** C1 and C4 can run independently.
After C1, C2 and C3 can run in parallel in separate branches, coordinating scope
helpers. C5 can proceed after C4 alongside C2/C3/C6. After C6, C7 and C8 can run
in parallel but coordinate `shell.js`; C9 comes near the end. C10 follows the
integrated result. #523 can independently build navigation with current routes;
coordinate shared helpers and serialize merges touching router/toolbar/brief.

### C1 acceptance and tests

- Status/diff/history/mutations address each source, including two repos with
  identical paths/status keys; changing source settings cannot redirect a save.
- Source-qualified git/files/terminal facts reach the right cache consumer;
  legacy primary facts remain compatible, secondary/plain roots are covered.
- Extend `bridge/src/app/tests/git/scope.rs`, scope/filesystem fixtures and
  `bridge/tests/api_contract.rs`; add named regressions
  `project_source_git_refuses_mixed_or_moved_scope` and
  `project_source_changes_keep_secondary_and_plain_roots_distinct`.
- Update Project Sources rows PS522-1/2/3/6; fixtures, capability and architecture
  changes land with the bridge. Existing Files scope tests remain green.

### C2 acceptance and tests

- Build-controlled sync and source mutations serialize without blocking app RPCs;
  stale path after lock wait refuses. Local commits and dirty files survive.
- A cut reports/include local base commits correctly; another checked-out branch
  does not silently become the new workspace base. #271 fast-forward path survives.
- Extend `bridge/src/source_sync/tests/`,
  `bridge/src/app/tests/configuration/source_sync.rs`; add
  `project_source_mutation_serializes_with_base_sync`,
  `workspace_cut_preserves_local_base_commits`, and moved-source wait regression.
- Verify PS522-4 plus Rift RI522-1. Temporary local remotes/fake helpers only.

### C3 acceptance and tests

- Create/list/attach/close each source independently, including plain sources;
  foreign source, mixed selectors and moved-location attaches refuse.
- Detach/reconnect preserves ID; tab close/EOF emits the right roster event.
  Source move/remove refuses with live shells; close then move succeeds.
- Extend `bridge/src/app/tests/runtime/terminals.rs` and `terminal_lifecycle.rs`;
  add `project_source_terminal_cannot_attach_sibling`,
  `project_source_terminal_survives_detach_not_source_retarget` and plain-source
  EOF push tests. Verify PS522-1/2/5/6 and contract fixtures.

### C4 acceptance and tests

- Fresh/migrated/restarted projects have exactly one stable pin; extra agents
  keep IDs/history/settings as workers. Worker and coordinator prompts/tool
  authorization differ by persisted purpose for every harness.
- Primary removal, forged-purpose creation and stale session callbacks cannot
  retire/reassign the pin or redirect work. Reset preserves its identity.
- Extend agent roster/store migration, project conversation and runtime-agent
  suites; add `project_roster_migrates_one_stable_coordinator`,
  `project_coordinator_cannot_be_removed_or_reassigned`,
  `project_worker_tools_follow_persisted_purpose`, and old-client projection cases.
- Verify CI522-1/2/3 and Task TS522-1; advertise/fixture `agents.projectWorkers`.

### C5 acceptance and tests

- New/existing worker assignment dispatches once to the exact same-project
  worker. A foreign project, obsolete generation or removed worker refuses.
- Workers can read/create their project's agents using coding tools without
  source-administration authority; pin protection applies through MCP too.
- Remove/reset worker A leaves B/pin untouched even with shared cwd. Source
  move/remove holds for live workers; child reclaim cannot touch the base owner;
  project deletion preserves source files and cleans only owned artifacts.
- Extend project agent tools/writes, tracker dispatch/assignee, removal/reset and
  `bridge/tests/api_contract.rs`; add `assign_project_worker_is_idempotent`,
  `project_worker_removal_preserves_shared_cwd_siblings`,
  `workspace_reclaim_never_reclaims_project_agents` and source lifecycle tests.
- Verify CI522-2/3, TS522-1/2 and RI522-2; MCP schemas/fixtures included.

### C6 acceptance and tests

- Project Changes works for every source; current branch/base/sync state is honest.
  Direct links preserve source/commit, diff-to-Files preserves source/path/line.
- Source add/remove/relabel/move reconciles without losing another source's draft
  or remounting chat; stale source mutation refuses. Adopt relevant #518 root
  reconciliation work rather than reimplement it. Older bridge paints cached
  supported content and never receives new params.
- Workspaces face lists named children only and shares keys/open targets with
  #523; project Tasks stays project-wide. Fix router comments in this change.
- Extend `projectViewDom`, `projectRailDom`, `projectRailState`, `router`,
  `projectFilesSupport`, `gitPane` suites. Add `projectChanges.test.js` and
  `browser/projectWorkspaceChanges.test.js` (390 and 1280 widths, light/dark,
  two Git sources + plain folder). Verify PS522-3/6.

### C7 acceptance and tests

- Terminal action opens existing console; no process starts on page load.
  New tabs honor explicit source; face/task navigation keeps current shell.
- Cold cached page, reconnect, source switch and same IDs on two devices cannot
  route input or roster events into another source. Close updates cache/UI.
- Extend `consoleModel`, `consoleDom`, `consoleAlias`, `cacheConsole`,
  `terminalReconnectIntegration`, `terminalFollowAuthority` suites; add
  `browser/projectWorkspaceTerminal.test.js` (390/1280, keyboard, source labels).
- Verify PS522-5/6 and accessible toggle state; no terminal-as-face route.

### C8 acceptance and tests

- One pin above separator, workers below, Add and names work, worker sends stay
  on exact conversations. Visiting a named workspace hides project workers.
- Project-hosted task keeps project rail; workspace-hosted task keeps its own.
  Same pin selected across surfaces retains canonical conversation/draft state.
- Assignee picker includes workers; older-client/cache capability fallback is
  pinned-only. Delete/reset/refusal/late-roster cases cannot alias survivors.
- Extend `agentRailModel`, `agentRailIsolation`, `agentRailDom`, `shellProjectRail`,
  `shellLinkedAgent`, tracker assignee and unread suites; add
  `browser/projectWorkspaceAgents.test.js` (390/1280 and keyboard) and a
  shared-cwd reset/draft race regression. Verify CI522-1/2/3, TS522-1.

### C9 acceptance and tests

- Apply the vocabulary table consistently to display copy, accessible names,
  prompt help and documentation. Do not rename protocol or persisted keys.
- Check every stale comment above was fixed by its owning change; coordinate
  #523's toolbar changes instead of making a competing breadcrumb.
- Extend `router.test.js`, `directoryRail.test.js`, `projectRailDom.test.js`,
  reference/legacy URL cases and #523 navigation tests where vocabulary appears.
  Document an explicit UI string audit; preserve `Projects` inbox face.

### C10 acceptance and tests

- Exercise the complete journey with fixture-backed browser/temporary-directory
  integration: edit/commit base, create worker, assign task, open per-source shell,
  switch to child, return, cut workspace containing local commits, remove worker,
  refuse live-source move, close/move, delete project preserving originals.
- Old/new bridge-client matrix, cold cache, reconnect, two devices with colliding
  IDs, secondary/plain/unborn sources and concurrent sync are covered.
- Add `browser/projectWorkspaceFlow.test.js`; run combined Rust integration and
  SPA gates, attach phone/desktop screenshots of Changes/console/agent separator.
  This is evidence for all added security rows; no live user browser or bridge.

Every implementation child follows repository TDD: observe its new regression
fail before making it pass. Bridge changes run test/clippy/fmt; SPA changes run
lint/full tests/build; contract changes run both API suites. Run gates under
`nice -n 10`, judge exits, scan changed code with Semgrep and history with
Gitleaks before push, and `git diff --check`. Each review updates the checklist
rows it implements with actual evidence; no planned row is marked verified early.

## Security checklist extensions

The matching checklist files carry these **pending** rows. Existing 100/100
scores describe shipped scope only; #522 earns 100/100 after every new applicable
row has evidence. This planning change does not claim implementation validation.

| Checklist | New row IDs | Required boundary |
| --- | --- | --- |
| Project Sources | PS522-1 through PS522-6 | Exact membership, captured location, cache/event separation, sync serialization, terminal lifecycle, source completeness |
| Rift Isolation | RI522-1/2 | Preserve isolated cuts; explicitly classify base workers as unisolated; never reclaim base/scratch as a child workspace |
| Chat Isolation | CI522-1 through CI522-3 | One stable pin, exact purpose/session authority, shared-cwd cleanup and no aliasing |
| Task | TS522-1/2 | Same-project worker assignment/MCP and truthful immutable task links/lifecycle |

No transport, pairing or vendor credential boundary is widened, so those
checklists do not acquire speculative rows. If implementing purpose-aware harness
cleanup changes a provider-specific path, revalidate the existing applicable
Codex App Server/Pi/Harness checklist controls in that child as well.
