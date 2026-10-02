# Workspace reviews

## Owner summary

A review records the committed work in every source directory of a workspace.
It lives on the task, with one saved branch, base and head per Git source.
Each directory has Changes and Files views inside the review.
Git Files shows the whole saved commit tree, including unchanged files.
Uncommitted files are counted and clearly excluded.
Non-Git directories say “Not a Git repository” and open on Files, labelled Live.
Pick a reviewer using the existing user/agent picker.
Any agent or model can review and finish the work.
Choose Merge, Push, or both for each source, with the destination you want.
An agent can instead use its own tools and mark the review complete.
Completion shows a short description, such as “merged API to dev; pushed web”.
Chosen steps succeeding, or an explicit completion report, finishes the review.
Deleting a source can make its review content unavailable.
Estimate: **6–8 focused engineer days**, in three increments.
Agents can use the first increment alone; the UI and Git buttons follow.

Plan only, based on main `ee49f20b93477a7959196d5853e3f0e5ba79bcd7` and the owner's
#89/c/tc-01M3Y803PF9GMSYA3RNNR6YRET decisions, with the Files-view correction in
#89/c/tc-01M3YADSQ09N48T9YB7QQF6524. Code references describe current
reuse points; the records and verbs below are proposed. No model allowlist,
reviewer/merger restriction, required-check machinery, candidate proof or
deployment gate is part of this feature.

## 1. Model: metadata for every workspace directory

Keep the task's identity, title, description, assignee and comment stream, as
#145 specifies. A task review names one workspace and has numbered snapshots.
Each snapshot includes every directory from that workspace's manifest, in order,
including later-added directories and directories that are unchanged or unavailable.
Do not enumerate only the project's current sources or use its first-source fields
as the entire review. `Workspace.directories` and `WorkspaceDirectory` in
`bridge/src/workspace.rs` already supply directory/source IDs, repo/source paths,
branch, base branch and provisioning status; `bridge/src/app/workspaces/directories.rs`
adds independent directories after workspace creation.

Store only metadata in `build.db`: review state (open/completed) and version,
snapshot ID/number/time/author,
per-directory identity and repository location (including a linked worktree's
common Git directory), branch, resolved base OID and head
OID, plus action results. Include the base's name/kind and the uncommitted-file
count for display. Non-Git or unavailable entries carry their reason, not invented
Git values. There are no saved patch/body tables or content digests.

### Snapshot and base selection

`snapshot` takes the task, workspace, expected review version and optional
per-directory base overrides. For each Git directory:

1. Read its committed HEAD and actual branch identity. A detached head has an OID
   without a branch; an unborn repository has no committed head.
2. Resolve the explicit base override if supplied; an invalid override is an error.
   Otherwise resolve the directory's `base_branch`, then its upstream if no base
   resolves, then the empty tree. Record and show the exact base used. Do not hide
   a base equal to HEAD behind an inferred alternative; the caller can override it.
3. Retain the head with `refs/build/reviews/<task>/<snapshot-id>/<directory-id>/head`.
   Ref components encode bytes outside ASCII letters, digits, `_` and `-` as
   `%HH` (including `%` itself). Actual directory IDs contain `:`, which Git
   forbids in ref names; metadata retains the original IDs (#328).
   Retain a non-empty base under the sibling `/base` ref too: it need not be an
   ancestor of HEAD. Directory IDs keep two sources sharing one repo distinct.
   Allocate a unique snapshot ID before pinning, independent of its display number,
   so concurrent calls cannot overwrite or clean up each other's refs.
4. Store the metadata and show “N uncommitted files not in this review”. An unborn
   source shows “No commits yet”; its working files are not a committed diff.

Snapshot creation is a synchronous RPC, with Git work off the app lock. It reads
one manifest and the per-source commit references, creates the refs, then commits
the metadata. This is a saved set of commit identities, not an atomic filesystem
snapshot. No body capture, capture budget or mid-capture retry is needed. A source
that cannot be read/pinned stays visible with its error; it is not silently omitted.
The final metadata write checks the review version; a losing call cleans up only
its own newly created refs, with their expected OIDs.

`diff` reads the saved base..head from Git, including requested file content when
needed. Reuse `bridge/src/diff.rs`'s commit diff machinery, extending its input
handling for an empty-tree base; do not call the live dirty-workdir diff. Current
workspace `git.changeset_diff` uses an unpublished-work baseline
(`bridge/src/app/git/mod.rs`, `changeset_subject`), so it is not this immutable read.

### Full Files view

The review includes a read-only Files browser for every saved directory, not
just files that changed. For Git sources, list the entire tree at the saved head
OID and read blobs from that tree. Opening an unchanged file requires no checkout
or navigation to the originating workspace. A source with no commits has an empty
committed Files view and retains the uncommitted-file note. Symlinks and submodule
entries are labelled without opening a live target.

For a non-Git directory, default to Files inside the review and show “Not a Git
repository” and “Live files — not saved with this review”. This uses the live-folder
default stated in #89/c/tc-01M3YAF5Z38PZ9XHA0SPTR5508; it is not an archived copy.
Keep the saved directory identity and resolve that specific source, without
falling back to another directory if it was removed. Missing sources show
“Source unavailable”. Changes displays the non-repository note and offers the
same embedded Files view.

Current `fs.tree`/`fs.read` in `bridge/src/app/fs.rs` read the filesystem;
`FsTreeParams` and `FsReadParams` in `bridge/src/api/v1/git.rs` have no commit
selector. Reuse these scoped reads only for the live non-Git view. Extend the
proposed review `diff` read with full-tree listing and blob-content modes for
Git sources, reusing listing/body/page shapes with read-only blob answers and
pages tied to that blob OID. Tree traversal at a saved OID is
new work, not a claim that current filesystem reads can serve historical files.

Refs survive branch movement and Git GC while their repository exists. They do
not archive a deleted repository. A deleted Rift/standalone source can leave the
old diff unreadable; retain its metadata/comments and show “Source unavailable”.
A removed linked worktree can remain readable by opening its recorded common
Git directory, rather than its deleted checkout path.
Release refs when their review history is explicitly deleted; closing or completing
keeps them. Existing store backup covers metadata, not the Git repositories.

### Comments and opinions

Task comments may carry a snapshot/directory/path/side/line anchor, a reply-to
comment, or an approve/request-changes opinion. Older anchors keep their original
snapshot. Display their original Git context when available, otherwise the comment
and an unavailable-context label. No stored line migration. Opinions describe the
snapshot seen; the picker routes the task and grants no exclusive rights.

## 2. Wire: five verbs

| Proposed verb | Purpose |
| --- | --- |
| `tasks.review.snapshot` | Synchronously save the workspace's per-source commit metadata and refs; return the review. Base overrides work from increment A. |
| `tasks.review.get` | Read review state, snapshots and per-source action results, plus available destinations. |
| `tasks.review.diff` | Read changed files/patches, the full tree at the saved head, or file content from Git, including unchanged files; reuse listing/body/page shapes. |
| `tasks.review.act` | Run the selected Merge/Push steps for named sources and record their results on the review. |
| `tasks.review.complete` | Record the actor and a brief action-taken description, and mark the review/task Done; no preceding Build action is required. |

Use existing task CRUD, assignment and comment verbs. Add optional anchor/opinion/
reply metadata to both `tasks.comment` and MCP `comment_task`, including their
schemas/adapters and shared persistence (`bridge/src/app/tracker/tools.rs`).
`bridge/src/tracker.rs` and
`bridge/src/store/tracker.rs::save_tracker_task_activity` are the metadata/timeline
seams; add the review tables through `bridge/src/store/schema.rs`.

Keep one review `version` check for stale mutations. An accepted write advances
it; `stale_version` makes the client refetch while keeping the user's draft.
Do not add request IDs, replay receipts or a separate operation lookup API.
Before each source's Git step, save a `running` action row; afterward save its
result. `get` and existing task invalidations expose those rows. Refuse a second
Git action on a source while its row is running. On restart, a still-running row
becomes “Interrupted: check and retry, or mark complete”. Nothing is replayed.

Thin RPC/MCP adapters call the storage/Git service. Accepted Git work runs without
a browser; action results are facts stored by that service. The SPA chooses the
steps and calls `complete` after they succeed, using recorded results after a
reconnect. The service has one completion writer for review completion and task
Done. `complete` calls it with a specific action description. An ordinary move
to Done by a user or agent calls the same writer for an open review, with the
mover as actor and “Marked done” as the default description, atomically with the
move. A review never refuses that ordinary task movement.

Add typed contracts in `bridge/src/api/v1/`, fixtures in `fixtures/api/v1/`, and
both `bridge/tests/api_contract.rs` and `spa/test/apiContract.test.js`. Increment A
uses wire **3.6.0** (`bridge/src/api/mod.rs`, `fixtures/api/versions.json`), announcing
its four verbs individually. `get` returns metadata/snapshots/completion in A;
available destinations and action results arrive with C. Gate snapshots, anchored
comments and Git actions separately so each increment works with older devices.

## 3. Selected Git steps

Each source row offers **Merge**, **Push**, or **Leave unchanged**. Merge and Push
are independently selectable ordered steps, not a third combined operation.
Merge selects a repository-local target branch; Push selects a configured remote
and full destination branch. Show the selected repositories and branches before
running. Resolve repository and checkout identities on the server, not from an
arbitrary caller path. A new remote branch is an explicit destination choice.

The action starts from the snapshot's head OID, and a differing live head is shown.
Merge uses that OID; a following Push uses the resulting merge tip. Push alone
uses the snapshot head. Uncommitted source files are neither staged nor changed.
For separate clones, import the selected OID into a temporary private ref in the
target repo; do not force-publish a same-named shared branch. Normal non-forced
pushes preserve Git's usual non-fast-forward refusal.

### Merge target rule

- If the target branch is checked out somewhere, merge in that checkout when it
  is clean and has no Git operation in progress. Otherwise refuse with the reason;
  do not switch branches, stash or reset the user's files.
- If checked out nowhere, create a temporary worktree **on the target branch**,
  merge there and remove that worktree while retaining the branch. Use normal
  branch-in-use checks, never a forced duplicate checkout. If another checkout
  acquired the branch, re-read placement and use the checked-out rule above.

Default to a merge commit. Conflicts return a per-source failure; abort only the
merge this action started and report an abort failure honestly. Private imported
refs and temporary worktrees are internal Git plumbing, not new review entities.
Use existing isolation/worktree ownership helpers (`bridge/src/isolation/worktree.rs`)
for temporary checkout creation
and removal, without invoking the workspace Done flow.
Never remove a user's target checkout. If cleanup fails, show the leftover path.

`bridge/src/source_sync.rs` already uses `SyncLock` per source checkout and can
move its base. Coordinate the selected source with that same lock and re-read the
target after acquiring it. Use the same configured source path as base sync, not
the temporary-worktree path. Test source sync against **both** merge paths, including
a target moving or becoming checked out. This lock coordinates participating Build
operations, not external shells; keep normal Git ref/index locks and branch-in-use
checks. Respect existing reclaim reservations.

Current helpers are reuse points, not ready-made review actions:
`bridge/src/gitgui/network.rs::push` chooses the live branch/upstream;
`bridge/src/worktree/mutation.rs::merge_into_base` is tied to a primary checkout
and publishes before checking it; `bridge/src/app/runs/review.rs::run_git_action`
can commit outstanding work and prune. Adapt lower Git helpers with explicit
source/target OIDs and structured arguments/deadlines (`bridge/src/git_process.rs`),
without inheriting those side effects. `bridge/src/isolation/rift.rs`'s shared-ref
force fetch is not the private import. The internal temporary merge path is new work.

Persist each step's result, including a merge's resulting tip, before starting
the next. If Merge succeeds and Push
fails, keep the merge result and retry only Push. If one source succeeds and
another fails, show both; do not roll back the successful repository. Interrupted
steps are inspected before an explicit retry. Any user or agent can instead use
other tools and call `complete` with a brief description. The result rows remain
visible alongside that report. No separate recovery state machine is needed.

## 4. SPA: the task's review across directory tabs

Add Create/Update review to workspace Changes and show the saved review on the
task page. Keep the task list, identity and timeline. Reuse the directory tabs in
`spa/src/views/workspaceChanges.js` and `spa/src/core/workspaceModel.js`, including
unchanged/non-Git/unavailable rows from the saved manifest. Do not reconstruct old
tabs from the live workspace. Each Git tab shows its resolved base/head and the
uncommitted-file note. Increment B adds the per-directory base picker.

Each directory offers **Changes** and **Files** without leaving the review. Git
directories default to Changes; their Files view browses the full saved head tree.
Non-Git directories default to Files with the non-repository and Live labels above.
Adapt `spa/src/core/fileRoots.js`, `fileTree.js`, `fileTabs.js`, `fileViewer.js`
and `pagedFileView.js` from `spa/src/views/files.js` through read-only review
adapters. The current Files view calls live `fs.tree`/`fs.read`; do not mount it
unchanged for Git snapshots or expose its editor. Keep the selected directory
when switching views, and let a diff's file link open that file in review Files.

Use `spa/src/core/changesReview.js`, `changesetBodies.js` and `diffRender.js` through
a snapshot adapter. Anchored feedback writes task comments, not the legacy run
conversation notes. Personal viewed-file marks stay personal. Reuse
`trackerAssigneePicker.js` and `trackerAssigneeControl.js` for reviewer selection.
The action sheet shows Merge/Push selections and per-source results; Mark complete
asks only for a brief action description. Add no separate PR surface.

Every view paints from cache. Review metadata/results, tree listings and file
reads write to the entity/body cache first; comments stay in the task/timeline
cache. Put review records under the named project entity with separate kinds and
task/snapshot/directory/path/side subkeys. `cacheSync.js` evicts unnamed entities;
do not invent an unregistered task entity or reuse live file addresses. Adapt
`cacheLifetime.js`'s file-head writer to the review kind. Reuse `localCache.js`, `bodyPages.js`,
`taskReadOrder.js`, `pushFence.js` and task invalidations in `cacheSync.js`. Cached
Git bodies are disposable; a missing repository is not repaired from live files.
Keep non-Git live file cache keys separate from immutable Git snapshot keys and
use existing live-file invalidation/refetch behavior. A late read for another
snapshot or directory may populate only its own cache entry, never the new view.
If a live refresh fails after cached content painted, show that failure or
“Source unavailable”; held bytes must not appear silently current.

Keep drafts and action selections in the separate `build-ui` store
(`localUiStore.js`), scoped to task/snapshot. Reconnect and tab changes preserve
them; stale writes refetch and repaint without automatically retrying Git actions.
Use current capability/refusal handling (`commandRefusal.js`). The bridge serves
records and executes requested Git; product choices and view state live in the SPA.

## 5. Agents

Expose snapshot/get/diff/act/complete through MCP to project and workspace agents
in the same project. They can read all source diffs and complete Git trees without
first checking out the branches, use existing scoped filesystem reads for live
non-Git files, leave opinions/comments and finish with a short description. Existing
actor authentication supplies attribution, not a model qualification test.

Reuse `assign_task` and the existing picker; a task note names the snapshot to
review. The picker may choose the user, an existing agent or the existing
create-agent options. Assignment is routing and retains current column behavior.
Update `bridge/templates/notes/task_tools.md` and relevant project/workspace notes
with this flow and the no-acknowledgments convention. MCP seams are
`bridge/src/mcp.rs` and `bridge/src/app/mcp.rs`. Automated handoff sequences and
workspace transfer remain #145's later work, not prerequisites for these tools.

## 6. Task integration

A task having a review record opts it into the new behavior. `complete` stores the
review's completed state, brief description, actor, timeline entry and task Done
in one metadata transaction. Task closure remains separate. Snapshot updates change
only the review; the SPA or agent explicitly moves/reopens the task when appropriate.
Neither taking a snapshot nor choosing a reviewer implicitly moves the task.

Increment A bypasses these existing automatic hooks **for review tasks only**:

- `bridge/src/app/tracker/activity.rs::move_held_task_on_complete` consumes its
  dispatched-task marker normally but skips `hand_held_task_on`'s automatic move
  to In review for a task with a review record.
- `bridge/src/app/workspaces/deletion.rs::remove_workspace` calls
  `activity.rs::close_tasks_of_finished_workspace` for merged Finish. That function
  must skip review tasks in its per-task loop, while ordinary linked tasks keep
  their existing closure behavior. Do not skip removal of the workspace itself.

Test both paths with review and ordinary tasks linked to the same workspace.
Review completion never invokes Finish/deletion. Existing lifecycle/reclaim is
separate; if it deletes a repository, old review metadata survives but its diffs
may no longer be available.

Implementation note (#328): project removal preserves task history too, so it
also keeps review pins. Explicit task-history deletion goes through the review
service to release pins before removing metadata. Ordinary moves to Done also
complete an open review with “Marked done”; `complete` remains the way to give
a specific description. Both use the same transactional completion writer.

## 7. Rollout

**6–8 focused engineer days**, including tests/review. Re-estimate after A.
Metadata-only snapshots remove most of the earlier capture/recovery work; target
checkout handling is the remaining uncertainty, covered in C's estimate. Full-tree
Git browsing and its file-read adapter add one day to A; B includes embedding the
existing Files components. There is no new archive or snapshot-body store.

| Increment | Deliverable and code areas | Verification / rollout | Size |
| --- | --- | --- | --- |
| A: useful agent reviews | Proposed `bridge/src/reviews/`, store/schema and typed API/MCP adapters. Four verbs: snapshot/get/diff/complete (`act` lands in C). All-source metadata/refs, base overrides/fallbacks, full Git tree/blob reads, action description and the exact legacy-hook guards above. | Shared-repo directories have distinct refs; concurrent snapshots cannot overwrite/remove winning pins; detached/unborn/missing bases; base override/empty tree; dirty files excluded; unchanged files/tree remain at saved head after live edits; paged blob reads; pin survives branch deletion/GC; deleted repo reads unavailable; version checks and review/ordinary task hooks. Bridge roll. Agents can read diffs/full files and complete using their own tools. | 2–3 days |
| B: review UI | Task/Changes and read-only Files adapters, saved directory tabs, base picker, anchored task comments/opinions in RPC and MCP, reviewer picker, cache/drafts in `spa/src/core/` and `spa/src/views/`. | Same paths in two repos, whole-tree browsing, Changes-to-Files navigation, non-Git defaults/Live label, old snapshot anchors, source removed after cache paint, two tabs/stale version, late reads after snapshot switch, review listings/pages survive reconnect and sync, mobile; comment RPC/MCP schemas/fixtures/capability. Bridge+app roll. | 2 days |
| C: selectable steps | `act`, review result rows, targeted Git helpers, temporary worktree handling and action sheet. | Checked-out clean/dirty targets; unchecked-out target/ref race; source sync contention on both paths; Rift import; Merge success/Push failure; interrupted row after restart; no automatic retry or workspace deletion. Temporary repos only. Bridge+app roll. | 2–3 days |

Implementation follows AGENTS.md TDD and affected-tier gates: Rust tests/fmt/clippy,
SPA lint/tests/build, wire fixtures, semgrep, gitleaks and diff-check. Those checks
validate this implementation; they are not a review-completion feature. Only this
plan document changes in #89.

## 8. Defaults

Git reviews cover commits only, with Changes and the entire committed Files tree.
Plain folders open their embedded Files view with “Not a Git repository” and
“Live files — not saved with this review”; general file archiving is outside this
release. This live-folder default follows the project agent's stated interpretation
of the owner's correction. Any agent/model may review or finish.
Merge defaults to a merge commit, Push is non-forced, completion retains
branches/workspaces, and source-repository deletion can lose historical diff access.
There are no outstanding owner choices blocking this draft.

## Workshop log: Opus read of 3143e6c4

The simplification is accepted. One Git detail in #4 is changed for the reason
below; the other seven requests are accepted as written.

| # | Decision and reason |
| --- | --- |
| 1 | Accept metadata-only Git snapshots and committed work only. Per-directory head refs retain commits; also pin a non-empty base because it may not be reachable from the head. Deleting the repository may lose the diff. |
| 2 | Accept five verbs and synchronous snapshot creation. Git file content is a diff read; results live on the review. |
| 3 | Accept version-only stale-click handling and running/interrupted rows. No request replay or recovery ledger. |
| 4 | Accept the target-placement rule; reject the detached-worktree plus raw ref-update detail. A checkout can acquire that branch after the placement check but before update-ref, leaving its files/index behind the moved ref. Use a temporary checkout on the target branch and normal Git branch-in-use checks instead; merge itself advances the branch. This also removes the separate ref swap. Test both paths with source sync; its path lock is not a lock on external Git. |
| 5 | Accept base_branch, then upstream, then empty tree; overrides ship in A, picker in B. Show the actual resolved base. |
| 6 | Accept exact per-task legacy-hook guards in A. Task movement on a new snapshot belongs to the SPA/agent. |
| 7 | Accept Merge and Push as separate selected steps, snapshot-head actions and a displayed differing live head. Remove the dirty-source dialog. |
| 8 | Accept the shorter plan, standalone A and stated non-Git default. Fresh estimate is 5–7 days; prior revision history remains in Git and the task timeline. |

The #4 race was reproduced in disposable repositories: a detached merge plus
update-ref left a newly checked-out target's files behind its HEAD. A temporary
checkout on the target branch refused a second ordinary checkout and kept its
index/files aligned when merged. This is normal Git protection, not a guarantee
against forced/manual ref writes by an external process.

### Final Opus signoff

Opus signed off on `fd106344` on 2026-10-02, including the temporary checkout
on the target branch. Its three final text edits are applied: increment A names
the later arrival of `act` in C; `complete` explicitly records the actor; and the
owner summary offers Merge, Push, or both. No further review round is requested.

### Owner correction: full workspace browsing

The owner requested Changes and Files inside the review for every directory in
#89/c/tc-01M3YADSQ09N48T9YB7QQF6524. Add complete tree/blob reads at each saved Git
head, including unchanged files, and embed Files rather than sending the reader
elsewhere. Non-Git sources default to Files with the non-repository and Live
labels, following #89/c/tc-01M3YAF5Z38PZ9XHA0SPTR5508; no frozen folder copy is
introduced. This supersedes the earlier “No Git diff” plus external Files-link
default. The estimate becomes 6–8 days. Opus's recorded signoff above covers
`fd106344`; this later owner correction is recorded separately.
