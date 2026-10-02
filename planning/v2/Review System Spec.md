# Workspace reviews

## Owner summary

A review is a saved snapshot of a workspace, attached to its task.
It lists every source directory and freezes each Git directory's changes.
Git sources keep their own repository, branch, base and head.
Updating the review takes another snapshot; older comments still have their context.
Pick a reviewer using the existing agent/user picker.
Anyone can comment or approve, with any model.
For each source, choose Push to a branch, Merge into a branch, or Leave unchanged.
Merge and push can be chosen together; the destination is yours to choose.
The action sheet shows exactly which repositories and branches it will change.
You or an agent can instead do the work separately and mark the review complete.
Completion shows a short description, such as “pushed API to main; merged UI to dev”.
The review is done when the chosen actions succeed or you or an agent mark it done.
There is no model allowlist, required-check setup or deployment gate.
Estimate: **6–9 focused engineer days**, in three usable increments.
Snapshots and agent completion ship first, then the review UI, then UI Git actions.

This redesign follows the owner's October 2 feedback on
#89/c/tc-01M3Y803PF9GMSYA3RNNR6YRET. It replaces the implementation design at
`bcdb1ba6`; that draft's Opus signoff does not apply to this revision. This is a
plan only. Current-code references below are checked against main
`ee49f20b93477a7959196d5853e3f0e5ba79bcd7`; bracketed references point to the evidence
index. Proposed tables, verbs and behavior are identified as proposals.

## 1. Model: the workspace is the snapshot boundary

Keep #145's task identity, title, description, assignee and comment stream.
Add review data to the task, not a separately numbered PR. One task review names
one workspace and has successive snapshots of that entire workspace. The workspace
can contain any number of source directories from the first release.

The old one-branch attachment could not represent this. `Workspace.directories`
contains independently identified directories; each carries `source_id`, `path`,
`source_path`, `base_branch`, `branch`, `is_git` and provisioning status.
`add_workspace_directory` can also add a directory that is not a project source.
A project row's top-level branch/path/remote describes only its first source.
Neither that row nor a single workspace branch name is a review manifest. [E1]

Proposed records in the existing `build.db`:

| Record | Contents |
| --- | --- |
| Task review | Task ID, workspace ID, current snapshot ID, open/completed state, version and completion description. No second title, assignee or reviewer authority list. |
| Snapshot | ID, workspace name at capture, author, time and ordered directory entries. Published snapshots are immutable. |
| Directory entry | Directory/source IDs, label, original path and repository identity, capture result; for Git, actual branch or detached HEAD, base ref, resolved base/head OIDs and diff-base OID. Every manifest directory gets an entry, including unchanged or unavailable ones. |
| Snapshot content | Immutable changed-file list, captured patches and bounded old/new file bodies, with content digests and explicit binary/omitted/unavailable markers. |
| Task comment | Existing body/actor/time, optionally a snapshot/directory/file/side/line anchor, reply-to comment, or approve/request-changes opinion. |
| Action result | Operation ID, snapshot, actor, selected per-directory action/destination and each result; brief action-taken description. Distinguish a bridge-run action from an agent's completion report. |

Snapshot the workspace manifest, not today's project template. A directory added,
removed or renamed later affects the next snapshot only. Two directories containing
`README.md` remain distinct. Reassigning the task never changes the saved workspace
or its content; #145's future workspace transfer can explicitly select another
workspace for the next snapshot.

### What is captured

For a Git directory, show its changes from its own fork point with its selected
base, including committed, staged, unstaged and non-ignored untracked work.
Record the resolved base and HEAD separately from the merge base. The existing
`diff_against_merge_base` supports this combined view, whereas today's workspace
Changes chooses an unpublished-work baseline; reuse the rendering and diff
helpers, not that moving baseline as the saved review's identity. [E2]

Resolve this per source, not from the project's first repository. A separately
added clone can have the same branch as its recorded base; an adopted directory
can lack a recorded base. In those cases use its upstream/publication baseline
and persist that baseline's kind and exact OID, or let the user select a base.
Never silently use HEAD as its own comparison base and hide committed work. [E2]

An unborn Git repository has no head yet: use an empty-tree baseline and show
its files as additions. A detached HEAD is a commit, not an invented branch.
A missing base or unrelated history gets an explicit entry explaining what could
not be compared; the user can select a different base and take another snapshot.
Plain directories have no Git base/head: keep an explicit “No Git diff” entry
with its recorded directory metadata and a link to live Files, labelled live.
Do not pretend a live folder read is saved review content. They have no push/merge
action. This release snapshots Git changes and records every directory; a general
non-Git file archive is outside it. Unavailable and non-Git sources must not
disappear because the first source happens to be a Git repository.

Use the existing diff exclusions and large-file behavior for Git sources; do not
follow symlinks outside a directory or capture ignored build output as review
content. List omitted/binary files and why their bodies are absent. Apply bounded
per-file and total capture budgets; an incomplete source is visibly incomplete,
never an empty successful diff. This is a review artifact, not a workspace backup.

Copy the captured text into snapshot storage. A later read must never reopen the
live file and present it as old snapshot content. Patches and displayed bodies
must come from the same captured content. Capture off the app lock; check
manifest membership and each directory's head/content version before publishing.
Retry a source that changes during capture or report it as unavailable. Publish
the manifest and collected content together in one store transaction. This is one
saved set of per-source observations, not an atomic filesystem snapshot across
several repositories. Each entry keeps its capture time/result.

Keeping content in the store makes old reviews readable after a rebase, branch
deletion or workspace removal. Git objects are not a substitute for captured
uncommitted text. Reuse normal store migration/backup and paged-body patterns;
no separate canonical Git archive is required. [E3, E6]

### Opinions and completion

The picker routes the task to a person or agent; it does not give that recipient
exclusive approval rights. Every agent in the project can comment, approve, perform an
action or report completion. Models, implementer identity and who approved do
not affect permission. Existing authenticated project scoping still identifies
who wrote the record; callers cannot supply a different author.

An opinion refers to the snapshot the reviewer saw. Updating a snapshot leaves
old opinions visible on that version. Approval and request-changes are useful
feedback, not prerequisites for a Git action or completion.

An anchored comment retains its original snapshot, directory, path, side and
line range. Replies use the same task timeline. A newer snapshot may display an
identical-content anchor in place; otherwise show “On snapshot N” with its
original context. No stored line migration or separate discussion service.

Completion records the current snapshot and a brief description. Successful UI
actions produce the description from their results. An agent supplies its own,
for example “merged server to dev; pushed web to release”. Do not demand SHAs,
command logs, approvals or proof of deployment before accepting it. The timeline
shows who reported it. Completion sets the review Completed and the task Done
in the same metadata transaction; it does not delete a workspace or branch.

## 2. Wire and persistence

Propose seven typed verbs under `tasks.review.*`:

| Verb | Purpose |
| --- | --- |
| `snapshot` | Create or update the task's review by capturing every directory in the selected workspace; optional per-directory base selections. Return capture operation/snapshot status. |
| `get` | Read review state, snapshot history/manifest and action results. Task identity and timeline remain existing task reads. |
| `diff` | Read a saved directory/file patch, using the existing changeset body/page shape. |
| `file` | Read a captured old/new file body; return its explicit omission marker when no body was saved. |
| `act` | Run the explicitly selected per-source Git actions against a named snapshot. Return an operation ID. |
| `operation` | Read capture/action progress and per-source results after reconnect or restart. |
| `complete` | Mark the review complete with the caller's brief action description. This works without a preceding Build-run Git action. |

Extend `tasks.comment` with optional snapshot opinion, immutable anchor and
reply-to metadata. Use existing task create/update/assign/move/close/reopen
operations for everything else. Do not add project policy configuration,
required checks, model evidence, candidate preparation or deployment verbs.

`get` also supplies available actions and server-resolved destination identifiers
for each live source; the action service revalidates them when called. Do not
accept arbitrary checkout paths or fall back to a first-source destination.

Snapshot replacement, action submission and completion carry the review version
and stable request ID. A stale version returns `stale_version`; the client
refetches and preserves the user's draft for an explicit retry. Repeating the
same request returns its previous result; it does not repeat a push. Ordinary
comments remain append-only and need no global review lock.

Extend the task's existing transaction to save review metadata and its timeline
entry together. Persist capture intent before off-lock work; publish its snapshot
and settle the operation atomically afterward. Interrupted capture returns an
interrupted result on that request ID; a retry is a new explicit request, not a
different snapshot silently assigned to the old one. Git side effects cannot
share a store transaction: save the selected action before starting it and save
the result afterward. The bridge service owns
only capture, storage and the requested Git operation; it never chooses a
reviewer, destination or next workflow step. Accepted work can finish headless.
Thin RPC/MCP adapters share those services. [E3, E7]

Use existing task change invalidations. `get` returns a coherent review version;
pulls write that version into the cache, then repaint. Add fixtures in
`fixtures/api/v1/`, update both contract suites and allocate the next wire minor
at implementation time; main currently uses **3.5.0**. Announce snapshot-review,
anchored-review-comments and Git-action capabilities separately so each increment
can roll independently, including B clients connected to A-only bridges.
[E6, E8]

## 3. Git actions: choose what happens to each source

The snapshot is reviewable without running Git actions. The action sheet lists
every source, its branch and the selected destination. Nothing defaults silently
to the project's first source or to origin/main.

| Choice | Behavior |
| --- | --- |
| Push to… | Choose a configured remote and destination branch for this source. Push the source commit with a normal non-forced push. A new branch is an explicit choice. |
| Merge into… | Choose an existing local target branch and its destination checkout. Merge this source into that branch; default to a merge commit to preserve history. No implicit push. |
| Merge and push… | The same merge, followed by push to the selected remote/branch. Show both destinations before running it. |
| Leave unchanged | Perform no Git operation for this source; include that choice in the result. |
| Mark complete | Record a brief description of work performed outside these buttons. Available to the user and every agent in the project. |

Push/merge acts on commits. When a snapshot contains uncommitted edits, say that
the action publishes committed work only and offer Open Changes to commit and
Update snapshot, or an explicit Continue with commits only. Never silently stage
or commit all the files just because the user chose Push. Before running, compare
the source HEAD with the displayed snapshot; if it changed, show the new state
and let the user refresh. This prevents a stale button from including unseen
commits; it is not an approval or check gate.
The action uses that exact commit, not a mutable branch name. With the explicit
commits-only choice, source dirt is left untouched and does not block the action.

Use workspace/directory identity to resolve each repository. Worktree and Rift
sources must both work, including a separately cloned directory added later.
When a target repository needs objects from a separate clone, fetch into a
private temporary ref instead of overwriting a shared branch of the same name.
Current Rift `publish` force-fetches shared refs, so it is not the new import
primitive. [E4]
Resolve a merge target ID to a server-discovered checkout of this directory's
recorded repository, distinct from the source checkout. Import the reviewed HEAD
into an operation-private ref, verify its OID, merge that OID and retire the
temporary ref afterward. A standalone clone with no separate target checkout can
still Push or be handled by an agent; do not invent a primary checkout for it.

The existing `git.push` uses the current branch/upstream, and
`merge_into_base` is tied to the primary checkout. `run.git_action` also commits
outstanding work and can prune a workspace. None is a drop-in implementation of
this action sheet. Add explicit source/target arguments to lower Git helpers;
reuse their process, ref validation and error handling without their run cleanup
or first-source assumptions. [E4]

Keep ordinary Git safety: validate refs/paths and selected remote, use structured
arguments and deadlines, and return dirty-destination/conflict/non-fast-forward
errors. Do not reset, stash or force-push automatically. Merge must confirm that
the selected destination checkout is clean, has the chosen branch checked out
and has no operation in progress; never switch or merge some other branch as a
side effect. Offer an existing suitable checkout or hand the operation to an
agent when the target has none. Serialize this service's operations on the same
target and honor existing busy/reclaim refusals. Other agents, terminals and base
sync can still write; use immediate target rechecks and Git's index/ref locks,
and report a competing write rather than claiming a repository-wide lock exists.
A merge conflict is a failed action with its directory named; preserve or abort
only the merge this action started and report whether cleanup succeeded. [E4]

For a push, use the explicitly chosen full destination ref and normal Git
fast-forward checks, not forced leases or a separately prepared merge candidate.
For merge-and-push, the merge result is merely the output of the chosen command;
there is no candidate entity, proof workflow or check admission. Remote hooks,
Git errors and existing CI still behave normally. A successful push completes
the selected action; waiting for CI, source-base sync or deployment is not part
of review completion.

Run a selected batch as separate per-source actions, with results persisted as
they settle. “API pushed; web merge conflicted” is a partial result, not an
all-repository transaction. Keep the review open if a chosen action failed;
retry only failed items after an explicit choice. When all chosen actions succeed
(including explicit Leave unchanged choices), record the summary and complete.
The user or an agent may instead resolve it separately and call `complete`.
For merge-and-push, retain the successful merge if the push fails; retry its push,
not the merge. Never roll back one repository because another action failed.

If the bridge stops after invoking Git but before saving its result, show that
action as Outcome unknown. Do not replay it automatically or claim success from
an unrelated local ref. The user or agent can inspect the result and mark complete,
or explicitly retry. Persist enough selected-source/destination detail to make
that choice intelligible. Block duplicate execution of a running operation ID;
a lost reply must not cause a second merge. An agent report is still accepted
without demanding recovery proof. Do not remove an already recorded uncertain
result when the agent completes the review. If a running operation settles after
manual completion or after a newer snapshot was created, update its own result
only; never reopen or complete a different snapshot. Manual completion does not
cancel an already running Git process.

## 4. SPA: one review across directory tabs

Add the saved Review to the existing task page and a Create/Update review action
to the workspace Changes surface. Use the task list with a review status/filter;
no separate PR list or numbering. The header shows workspace, snapshot time,
reviewer/assignee picker, opinions and the latest action description.

Reuse the workspace's per-directory tab pattern. Each tab belongs to the saved
manifest and shows its own branch/base/head, file list and capture status.
Removing a live directory does not remove its old review tab. A mobile layout
uses the same directory selector and existing diff renderer. Reuse
`changesReview.js` through an immutable snapshot adapter, replacing its legacy
conversation-note submission with anchored task comments. Existing viewed-file
marks remain personal UI state, not approval. [E5]

The review controls are Update snapshot, Choose reviewer, Approve, Request
changes, Actions and Mark complete. The Actions sheet presents one row per
source, an action picker and the destination fields that action needs. It shows
success/failure beside each source while work runs and the resulting short
summary afterward. It does not ask for model policy, check commands, rollout
coverage or deployment evidence.

Every view renders from the cache. Task comments stay in the existing task/timeline
cache; the review cache holds snapshot manifests, results and paged bodies, written
before display. Key saved
bodies by device/task/snapshot/directory/path/side; never share the live Changes
body key. Reuse task read ordering and push fencing so an older response cannot
replace a newer snapshot. A second tab seeing `stale_version` refetches and
repaints; it never silently resubmits a destructive action. [E6]

Keep unsent comments and action selections in the separate `build-ui` store,
scoped to task and snapshot, following current draft lifetime rules. Existing
task drafts are retained; extend/test `uiDraftLifetime.js` explicitly if the new
address shape needs pruning, rather than assuming it already supports it. Reconnecting
or changing directory preserves them. A new snapshot leaves the old draft
explicitly on its original snapshot rather than attaching it to different lines.
Capability/error handling follows current cache-backed controls and
`commandRefusal.js`; no connection-dependent replacement of the review view.

## 5. Agents and reviewer choice

Reuse the task assignee picker and `assign_task`: user, project agent, existing
workspace agent, or the existing create-agent choices. The chosen agent receives
the task and snapshot reference and can read all source diffs without checking
out the branches first. The note names the snapshot; it grants no special
approval or merge authority. [E5, E7]

Expose MCP equivalents of snapshot/read/opinion/action/complete to project and
workspace agents in the same project. No model allowlist, configured-versus-
reported model question, implementer exclusion or “project agent only” merger.
An agent the user asked can do the work, use its ordinary Git/deployment tools
as needed, then report the brief action taken. Do not make it reconstruct Build
operation receipts to be believed.

Update `bridge/templates/notes/task_tools.md` and relevant workspace/project
notes: take/update a workspace snapshot, post feedback on the task, pass the task
through the picker/assignment tools when useful, and complete with a short action
description. Post on meaningful changes, not acknowledgments. Automatic handoff
sequences and moving checkouts between workspaces remain #145's later work;
this release adds neither a workflow engine nor a prerequisite assignment-system
rewrite. Existing assignment reliability work can proceed independently. [E7]

## 6. Task integration and completion

The task is still the unit of work. Review creation links the workspace and
persists its snapshot; it does not force every task into a review. A saved review
record is the opt-in for the new behavior.

On explicit review completion, save review state, description and task Done
together. Do not wait for an additional user approval, checks or deployment.
Task closure remains an existing explicit action, separate from the board column.
Updating a completed review explicitly opens a new snapshot cycle and moves the
task back to In progress; its old completion stays in history. Merely commenting
or choosing a reviewer does not reopen completed work.

For review tasks, generic conversation Complete must not move a completed review
back to In review, and merged workspace Finish must not complete/close unrelated
reviews without an action description. Bypass those legacy automatic task hooks
when the task has a saved review record. Ordinary tasks keep their current
behavior; test both paths. Assignment retains its existing task-column behavior
and does not change review state; moving a task In review remains explicit. [E7]

Do not run workspace Finish/deletion as part of review completion. Existing
workspace lifecycle/reclaim remains separate; a task becoming Done may trigger
its normal sweep. Saved review content must remain readable afterward. [E1, E7]

## 7. Rollout and estimate

**6–9 focused engineer days**, including implementation tests and review. This is
a fresh estimate for workspace snapshots and selected actions, not the earlier
11–17-day gated-merge design. Re-estimate after the first usable increment;
bounded content capture and adapting target selection are the main uncertainties.

| Increment | Deliverable and main files | Verification and roll | Size |
| --- | --- | --- | --- |
| A: saved workspace reviews | Proposed `bridge/src/reviews/` service, `store/reviews.rs`, `api/v1/reviews.rs`; extend tracker/store schema, MCP and fixtures. Capture all directories, immutable content, get/diff/file/operation, direct completion and task hooks. | Worktree/Rift/plain/unborn/detached/removed-directory fixtures; standalone-clone base selection; capture races/limits and interruption, store restart/backup, duplicate completion, task Done/legacy paths. Bridge roll. Agents can read/review and complete work using their own tools. | 2–3 days |
| B: review surface | Task page/Changes adapter, directory tabs, anchored task comments and opinions, existing picker, cache/draft integration in `spa/src/core/` and `spa/src/views/`. | Two different repositories with the same file names, snapshot updates after rebase, old anchors after deletion, two tabs, reconnect/cache-only paint, mobile picker/diff. Bridge+app roll for comment fields. Usable review with manual/agent actions. | 2–3 days |
| C: selectable Git actions | Explicit source/target Git helpers, `act` and extended operation results, per-source action sheet and completion summary. | Two remotes; selected target checkout; Rift private-ref import/cleanup; standalone clone; dirty-source commits-only leaves edits untouched; wrong target/dirty destination/conflicts; merge success then push failure/retry; lost reply/restart and source movement. Temporary repositories only. Bridge+app roll. | 2–3 days |

No first-release work on model policy, required checks, candidate proofs,
deployment coverage, range approvals, automatic checkout transfer or cleanup.
Multiple sources are included in A, not deferred behind a single-source release.

Implementation follows AGENTS.md TDD and affected-tier gates: Rust tests,
fmt/clippy; SPA lint/tests/build; wire fixtures; semgrep, gitleaks and diff-check.
Those are development checks for shipping this feature, not user-configured
requirements to complete a review. This planning change runs documentation scans,
not product tests. The security review covers input/path scoping, captured content,
Git command arguments and duplicate operations without creating user approval policy.

## 8. Decisions and remaining questions

The owner's decisions are settled: a workspace snapshot across all sources;
selectable actions; any agent/model; reviewer picker; completion on chosen-action
success or explicit agent report. The configured-model question disappears with
the allowlist. No approval is being sought again for those choices.

Working defaults for the draft: include uncommitted changes in the saved view;
keep ignored files and MCP configuration out; retain old snapshots/comments; use normal
non-forced pushes and merge commits in the UI; leave workspace cleanup separate.
These follow the existing Changes behavior and keep actions explicit.

One question is posted on #89: should plain, non-Git folders also have saved file
contents in release 1? Recommendation: keep their explicit “No Git diff” entries
and labelled live Files links; save Git changes now and treat a general folder
snapshot as separate scope. That recommendation is the basis of this estimate,
not an owner decision already received. The Opus read should check this boundary,
action clarity and the smaller estimate.

## Revision history

- `2942c209` preserved the September Astra/Opus work. It established durable
  review context and discussed Git operations but predated tasks and today's
  workspace/source handling.
- The October 1 rounds at `700e642e`, `49aa3469`, `52edfe58` and `bcdb1ba6`
  narrowed a policy-heavy design and reached Opus signoff. Their one-branch
  model, mandated main push, reviewer restrictions, model policy, candidate/check
  admission and deployment gate are superseded, not implementation requirements.
  Detailed historical decisions remain in those commits and #89's timeline.
- **October 2 owner redesign:** accepted every choice in
  #89/c/tc-01M3Y803PF9GMSYA3RNNR6YRET. The code confirms that a workspace is a
  directory collection, so snapshot identity moves to that collection. Completion
  becomes an action result or attributed report. Remove the policy machinery
  rather than retaining it behind defaults. A new Opus read is pending.

## Current code evidence

All paths below exist on main `ee49f20b93477a7959196d5853e3f0e5ba79bcd7`.
They identify reuse points and limitations, not already implemented review features.

| Ref | Current code and its implication |
| --- | --- |
| E1 | `bridge/src/workspace.rs` (`Workspace`, `WorkspaceDirectory`); `bridge/src/app/projects/mod.rs` (`ProjectSource`); `bridge/src/app/workspaces/directories.rs`; `bridge/src/mcp.rs` (`add_workspace_directory`); `ARCHITECTURE.md`, Projects and sources / Workspaces and worktrees. Workspaces contain separate Git/plain directories, including ones added after creation; a project's first-source convenience fields are insufficient. |
| E2 | `bridge/src/diff.rs` (`diff_against_merge_base`, `diff_between_commits`, dirty-workdir options, MCP exclusions and `LARGE_FILE_BYTES`); `bridge/src/app/git/mod.rs` (`changeset_subject`, `resolve_workspace_git_scope`); `bridge/src/gitgui/unpushed.rs`. Existing diff rendering includes dirty/untracked work; workspace Changes uses unpublished-work scope. Durable whole-workspace capture is new. |
| E3 | `bridge/src/tracker.rs` (`Task`, `TaskComment`); `bridge/src/store/tracker.rs` (`save_tracker_task_activity`); `bridge/src/store/schema.rs`; `bridge/src/store/operations.rs` (`backup_to`). Existing task/timeline/store transactions and backup; review content tables and atomic completion are proposed. |
| E4 | `bridge/src/gitgui/network.rs` (`push`); `bridge/src/gitgui/branches.rs` (`ref_list`); `bridge/src/worktree/mutation.rs` (`merge_into_base`); `bridge/src/isolation/rift.rs` (`publish`); `bridge/src/app/runs/review.rs` (`run_git_action`); `bridge/src/git_process.rs`; `bridge/src/remote_url.rs`; `bridge/src/source_sync.rs`; `bridge/src/app/workspaces/reclaim.rs`. Present Git actions are checkout/run oriented, not snapshot/per-source action batches. Source sync and reclaim already coordinate their own writers. |
| E5 | `spa/src/views/workspaceChanges.js`; `spa/src/core/changesReview.js`, `changesetBodies.js`, `diffRender.js`; `spa/src/views/trackerTaskView.js`; `spa/src/core/trackerTaskPage.js`, `trackerTaskRender.js`, `trackerTimeline.js`; `spa/src/core/trackerAssigneePicker.js`, `trackerAssigneeControl.js`, `trackerAssignee.js`. Existing per-directory tabs, diff UI, task timeline and actor picker are the UI seams. |
| E6 | `spa/src/core/localCache.js`, `localUiStore.js`, `uiDraftLifetime.js`, `cacheSync.js`, `taskReadOrder.js`, `pushFence.js`, `bodyPages.js`, `commandRefusal.js`; `bridge/src/body_page.rs`; `bridge/src/changes.rs`. Cached bodies, separate build-ui drafts, read ordering and invalidations exist; review snapshot cache keys are new. |
| E7 | `bridge/src/mcp.rs`; `bridge/src/app/mcp.rs`; `bridge/src/app/tracker/dispatch.rs`, `activity.rs`, `notices.rs`; `bridge/src/app/workspaces/deletion.rs`; `bridge/templates/notes/task_tools.md`; `bridge/templates/project_agent.md`. Existing assignment/delivery, Complete→In review and merged-Finish close behavior need deliberate integration; none defines the new review's authority or completion policy. |
| E8 | `bridge/src/api/mod.rs` (`API_VERSION = 3.5.0`); `bridge/src/api/v1/mod.rs`, `tasks.rs`, `git.rs`; `fixtures/api/versions.json`; `bridge/tests/api_contract.rs`; `spa/test/apiContract.test.js`; `AGENTS.md`; `ARCHITECTURE.md`. Typed additive wire changes, cached rendering and the bridge-service boundary remain required. |
