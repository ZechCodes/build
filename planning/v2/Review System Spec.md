# Local reviews

Implementation plan for #89, revised 2026-10-02 from the Astra / Opus 5.5
workshop of 2026-09-22, commit `2942c209e39988e216b4e4ed1166adf3cb62b882`.
The original two rounds are historical agreement; **this revision awaits a new
Opus challenge**. Product choices remain in section 8. No product code or
implementation/deployment authorization is delivered by this document.

Code baseline: `origin/main` at
`adf08b062c741723addb1b861ce434f573b87512`, fetched 2026-10-02 UTC. References
[E1]–[E12] below describe that exact code, not a live branch. Everything under
sections 1–8 is a **proposal** except explicitly identified current behavior.
Read `AGENTS.md` and `ARCHITECTURE.md` first; code wins where historical specs
or even the architecture's version summary disagree.

The task is the review. Attach committed local branches to a task, review their
changes together, leave anchored feedback in its existing timeline, pass the
work to another workspace, and integrate the reviewed revisions. There is no
second PR identity, number, title, assignee, or comment stream. This applies
Zech's later direction in #145 to #89's original eight-section brief. A review
round and its branch snapshots are child records of the task, not another
product object. GitHub and a network remote are unnecessary.

Zech's requirement preserved from #89's second workshop: deployed work needs
review by Zech or an independent agent of the configured qualifying model
class. Implementer self-approval never counts. #145's timeline correction is
also settled: **human-in-the-loop is a step type**, not a pause imposed on every
step. #86's later correction puts work that requires an always-on process in
isolated bridge services; the SPA owns presentation and connected interaction.

## What was stale in the September plan

| September assumption | Current evidence / revised decision |
| --- | --- |
| Separate PRs, `PR #N`, a Reviews project tab, PR comments beside issue comments | Superseded by #145: task identity and timeline, task Changes/review surface, multiple attached branch proposals. No PR-to-task join or second numbering sequence. |
| A review request must not reassign the implementation issue | Superseded by #145: explicit handoff assigns the same task to its reviewer, back to its implementer for fixes, then to its deployer. Contributor identity persists independently of the current assignee. |
| Every step waits for a human, or workflow runs only in a connected SPA | #145/c/tc-01M3A6P0V88S483NXWH48CP83K makes human an explicit step type; #86/c/tc-01M37YEQS990896PCGFJV7KYGA keeps always-on work in bridge services. |
| API 1.12.0; `issues.*`; `Issues Spec.md` | API is **3.4.0**, tracker is `tasks.*`, spec is `Tasks Spec.md`. The leading 3.2.0 sentence in `ARCHITECTURE.md` lags its own later notes and code. `api/v1/` paths still exist. [E1] |
| Unknown top-level request params are silently discarded | `parse_params` now refuses unknown top-level fields; nested types still need explicit validation. Named capabilities remain required. [E1] |
| `tasks.link` is an available wire primitive | Removed from the wire in 3.0; `link_task` MCP still exists. Add explicit task-review attachment primitives, not calls to removed verbs. [E2, E7] |
| `workspace_finish_legacy` drops `action: delete` (#87) | Fixed: it forwards the selected branch into `finish_workspace`. Do not plan to fix it again. [E4] |
| Every workspace finish closes all linked tasks | Too broad now: closure is conditional on a merged run. Still unsuitable for review cleanup, which must not close unrelated tasks. [E4] |
| Cleanup needs a new independent lifecycle system | Reclaim, reservations, path containment and checked branch deletion now exist. Extend these for local integration evidence; preserve remote-based behavior for old callers. [E4, E5] |
| The review target has no other background writer | Source-base synchronization now fetches/fast-forwards bases. It has its own path lock; review landing needs coordinated reservations, not a second independent mutex. [E6] |
| Reconnect reads first and subscribes later; drafts share the evictable replica | Current sync repairs late subscription coverage and fences pushes; drafts/UI state have their own `build-ui` store. Extend those seams, do not rebuild them. [E9] |
| New active review UI can extend legacy `taskReview.js` plan approval | Singular `task.*` is historical plan data; several mutations were removed. Tracker tasks use `trackerTaskView.js`. Keep legacy browsing separate. [E1, E8] |
| `templates.rs` is the only prompt source, and model configuration proves the model that reviewed | Prompt text is in `bridge/templates/`; live model observations need capture and validity checks. Configured choice alone is insufficient. [E7] |

Retained from the two earlier rounds: immutable H/T/B snapshots; private local
refs for Rift imports; no-ff candidates; exact-candidate checks; a small Git/DB
journal; conservative line anchors; qualified independent review; reviewed
release coverage; explicit completion and cleanup. The sections below adapt
those decisions rather than reopen them as a blank design exercise.

## 1. Model

### Task identity, branch attachments and persistence

Reuse the tracker task's ID, `#number`, title, Markdown body, author, assignee,
open/closed state, board column, labels, links and timeline [E2]. A task's
`review` extension holds its draft/open intent and active round; it does not
replace the task's board column or create another independently assignable item.
Legacy `links.branches` strings remain useful navigation but cannot identify a
repository or a reviewed revision. Never silently convert them into approvals.
Existing task `attachments` are uploaded files [E2]; name the new field
`branch_attachments` and do not repurpose those upload records.

| Task-owned record | Proposed fields / meaning |
| --- | --- |
| Branch attachment | Stable child ID, task ID, repository binding, original workspace/directory, authoritative source checkout and full source/target refs, implementer identities, current immutable revision ID, active/retired flag. One source branch/target per attachment; a task may have several repositories. |
| Repository binding | Persistent repository ID and canonical registered source identity; verified Git common directory and independent-clone provenance; explicit authoritative local target repository/ref. Device paths are locators derived from registered scope, never arbitrary caller authority. |
| Revision | Immutable ID/sequence, attachment ID, source `H`, observed target `T`, merge base `B`, source/target refs, actor/time, retained object refs. Commits are `B..H`; files compare trees `B` and `H`. |
| Review round | Task-owned immutable scope ID containing the complete ordered set of required attachment/revision IDs and the implementation contributor set. Adding/removing/retargeting an attachment or publishing a different H creates a new round; it never broadens an existing approval. |
| Submission | Append-only task event with round ID, reviewer actor, decision (`comment`, `approve`, `request_changes`, `withdraw`), rationale comment ID, timestamp and bridge-captured session/model facts. A submission covers the entire named set; partial findings can be comments, not an implicit whole-task approval. |
| Thread/anchor metadata | Child thread ID, task/round/revision, immutable line/hunk/file anchor, optional reply-to, open/resolved metadata and version. The text itself is a `tracker_comments` record; replies are also ordinary task comments. |
| Workflow instance | Task ID, configuration version, ordered bounded steps, current step ID/generation, input round, prior implementer, explicit recipient/workspace and transitions, status, receipts. Each step is `agent` or `human`, with a purpose such as implement/review/deploy. Not the retired plan/stage scheduler. |
| Candidate / check / operation | Attachment/revision, exact H/T/C/tree, check execution IDs, producer and raw exit/signal, journal status, explicit evidence/configuration versions, outcome and timestamps. No separate PR lifecycle. |
| Delivery / completion evidence | Task timeline events referring to handoff receipt, merge SHA, exact release SHA, deployment/verification outcome, and explicit task completion intent. |
| Project review configuration | Versioned exact qualifying model-ID allowlist, user-only writes; selected workflow/check requirements stored separately from display. No model names hard-coded into admission code. |
| Retrospective review | User-only exact repository/from/to/tree range approval and withdrawal, recorded as evidence on a designated task so uncovered work also has one discussion surface. |

Use the existing `build.db` (default `~/.build/tasks/build.db`) and additive
child tables beside `store/tracker.rs`, with query keys hoisted like tracker
rows [E2, E3]. Proposed tables hold branch bindings, revisions, rounds,
submissions, thread metadata, workflow steps and Git operations; **no PR table
or parallel review-comment table**. Persist task change, canonical comment/event,
aggregate `review_version`, caller operation receipt and explicit next delivery
intent in one SQLite transaction. Extending store transaction APIs is work to
implement; current tracker writes do not already guarantee that whole set.

Task storage uses the canonical project path, mapped to live `proj-N` IDs on
reads [E2]. A source ID or path alone must not silently rebind retained history
after a repository replacement. Persist a repo binding, validate it on each
operation, and mark missing/replaced repositories unavailable. Explicit project
relocation/rebinding is later work. Attachments survive workspace deletion.
Review data participates in project deletion/backup policies; no orphaned child
rows, pins silently dropped by source removal, or hidden global archive.

Retain immutable Git refs in the canonical project source repository, e.g.
`refs/build/task-reviews/<task>/<attachment>/<revision>/{head,target,base}` plus
candidate refs. Import and verify connectivity without depending on alternates
into disposable workspaces. Install/verify pins before publishing the DB
revision; journal and reconcile interruptions. Retain published history in the
first release. A backup must cover a consistent SQLite snapshot and the source
repositories/private refs; DB backup alone cannot restore historical diffs.
Do not add a second bare archive. Loss of the canonical repository leaves
metadata readable and Git bodies explicitly unavailable.

### States and review validity

The six requested display states are task-review projections, not new tracker
columns: Draft (review not opened), Open (opened without a qualifying decision),
Changes requested, Approved, Merged (every required attachment of that round
integrated), Closed (task closed with unmerged work). Preserve merged history
when a task later closes or reopens. Show partial integration and deployment as
separate facts; a partially merged multi-repository task is never simply Merged.

Use latest submission per reviewer on the exact round. A qualifying current
approval wins the aggregate Approved label; another reviewer's disagreement
remains prominent but adds no veto that Zech has not asked for. A relied-on
approver's withdrawal/request-changes removes their approval. Thread resolution
is independent. Review admission requires opening the draft explicitly.

At merge/deploy admission require approval by the authenticated user or an
agent outside the accumulated implementer set whose captured model qualifies
under the current project configuration. User self-review remains allowed;
agent self-review does not. Create/publish callers and explicitly named
contributors accumulate; reassignment to a reviewer never erases implementers.
A project agent importing a snapshot on behalf of another agent records both
publisher and declared contributors. Git author strings and same-workspace
membership do not establish contributor identity.

Preserve the prior plan's exact-model rule, but do not present its September
sample model IDs as today's approved defaults. Initial configuration must be
explicitly saved by the user from current model discovery. A fallback catalog
or role preference is not evidence of the runtime model. Capture agent ID,
session generation, selected model, observed model, provenance and uncertainty
at submission; refuse qualification on unknown/fallback/mismatch evidence.
The current observations are a starting seam [E7], not a ready-made durable
review attestation. Missing config or facts requires user/qualified review.
MCP cannot create user approvals or change the allowlist.
In particular, the current agent digest falls back from unobserved runtime
model to configured choice. Capture the underlying observation, not that
display projection, and persist a session-instance identity across restart;
today's live session generation alone is process-local [E7].

Snapshot qualifying evidence/config version at admission, and serialize relevant
review/config edits with the short apply admission window. Updates before Git
application invalidate admission; later withdrawals retain the merge fact and
can invalidate pending deployment coverage. Do not hold the AppState mutex
while Git runs. This is a workflow correctness guard, not an adversarial sandbox:
same-user shell/database access can bypass Build's interfaces.

Authors explicitly publish committed fixes. A new H, retargeting, or a changed
attachment set starts a new round requiring re-review, even for an identical
patch after rebase. Ordinary target advancement preserves the unchanged source
review but needs a new candidate and new checks; keep original B/T for the
historical diff. Title/body edits retain code approval and get visible task
history; changing scope requires a new round. An empty/already-contained source
is a fact, not an invented merge. Dirty edits stay outside the snapshot.

Prevent competing active attachments from claiming the same registered
source-checkout/ref/target tuple under different tasks. Related tasks link to
the owning task instead of duplicating reviews. Multiple attachments are not an
atomic multi-repository transaction; first release allows at most one required
attachment per target repository/ref in a round, avoiding an implicit stacked
merge protocol. Same-name branches in independent clones are distinct bindings.

### Line and hunk anchors

An anchor stores `{round_id, revision_id, old_path, new_path, side, blob_oid,
start_line, end_line, context_digest}`. Positive inclusive lines name the blob,
not rendered patch rows. Hunk feedback stores the exact old/new ranges and patch
content key; it must not attach a hunk ordinal to a moving diff. Validate blob,
path, side, range and task membership against retained objects. Additions use
the new side, deletions the old; renames retain both paths. Read symlinks as Git
blobs, not filesystem targets. Binary/submodule/capped text gets file-level
feedback with an explicit reason line feedback is unavailable.

The original anchor is immutable. Carry its placement forward only when path,
side and blob OID remain identical. Otherwise show “Outdated — revision N” and
open the original diff; replies/resolution still work. No guessed line migration.
Store comment text once in the task timeline with thread/anchor metadata; the
diff view projects those same records. Draft comments use `localUiState` keyed
by device/task/round/revision/anchor [E9], separate from evictable replicas.
Current task file refs carry only path/line ranges, and the timeline renderer
does not render them as immutable review anchors [E8]. Add the typed anchor and
its timeline rendering; reusing the comment text alone is not enough.

## 2. Wire

Extend plural `tasks.*` with a typed `tasks.review.*` subfamily in a new
`bridge/src/api/v1/task_reviews.rs`; keep adapters thin. Names below are
**proposed**, not callable today. Reuse existing task CRUD/assignment/comments
where their semantics match [E1, E2]; do not revive singular legacy `task.*`.

| Method / family | Inputs and primitive result |
| --- | --- |
| Existing `tasks.list`, `tasks.get` | Add capability-gated compact review summary/current round. Existing list remains the review list; add review filtering/paged detail only with declared new semantics. |
| Existing `tasks.create`, `tasks.update`, `tasks.close`, `tasks.reopen` | Task identity, description and lifecycle. No `reviews.create`/separate close API. Legacy callers continue their documented behavior. |
| `tasks.review.attach`, `.retire_attachment`, `.open`, `.publish` | Task, registered workspace/directory/repo binding, full refs, expected H/T, aggregate version and operation ID. Pin immutable revisions and explicitly create/open a round; never auto-commit. |
| `tasks.review.get`, `.timeline` | Current round/evidence block plus independently paged task timeline; current state is not reconstructed from the first history page. Keep old `tasks.get` whole-history response for callers that did not opt into paging. |
| Extended `tasks.comment` | Optional review anchor/thread/reply metadata, caller-stable comment ID and expected review/thread version. Stores an ordinary task comment and invokes the same task notice path. |
| `tasks.review.resolve`, `.submit` | Thread resolution, or exact-round decision with rationale; authenticated actor/model capture and expected version. Append events and task comment in one transaction. Late historical submissions need explicit historical intent and never advance current work. |
| `tasks.review.handoff`, `.workflow_get`, `.workflow_set`, `.advance` | Expected step/generation/round/assignee; explicit recipient and destination workspace plus step configuration. Durable checkout/delivery receipt; reused task assignment service. Human advancement only by the user on the current human step. |
| `tasks.review.commits`, `.files`, `.diff`, `.file` | Attachment/revision and bounded cursor/body range. Immutable OIDs/content keys, completeness/truncation; no arbitrary filesystem/ref access. |
| `tasks.review.prepare_merge`, `.record_check`, `.checks`, `.merge`, `.operation` | Exact revision/H/T/C and config/version/evidence IDs; candidate or conflict facts, attributed check executions, durable merge result and recovery status. |
| `tasks.review.settings_get`, `.settings_set` | Project exact model allowlist and version; writes only from authenticated user context. |
| `tasks.review.coverage`, `.approve_range`, `.withdraw_range` | Registered repository, trusted deployed baseline and exact release range/tree; complete coverage facts. Range approvals attach to a task and are user-only. |
| `workspace.cleanup_integrated` | Explicit local-only cleanup mode with per-directory target binding/current expected heads and stable operation ID. Reuses lifecycle guards/removal, never closes linked tasks. |

Every new mutation gets caller-stable operation identity bound to actor,
project, input hash and versions. Same ID/same body returns the receipt; same
ID/different body conflicts. Do not assume current `INSERT OR IGNORE` comment
persistence proves content-sensitive idempotency. New nested types reject unknown
safety fields explicitly as well as current top-level strictness [E1, E3].

`get` returns a consistent bounded current block: attachment set, H/T/B per
revision, latest decisions/model facts, contributors, step, candidates/check
IDs and operation states; growing history is separately paged. Define limits
(e.g. 20 attachments and 50 current reviewers per round) and **refuse writes
exceeding them** rather than truncate the authoritative approval set. Logs are
paged/capped (16 KiB inline tail); task text uses the existing 32,000-byte bound.
Repository/commit-indexed checks update the current task projection only when
that SHA is part of its active round/candidate. Missing/incomplete evidence
cannot mean approved or passing. Decisions are raw facts plus config; SPA owns
labels while the service rechecks authoritative admission.

Reuse the project-scoped `tasks` change kind [E2, E10]. Each child write bumps
aggregate `review_version` and notes its task after commit; do not add an
independent reviews stream. Subscribers refetch the affected review/current
block and membership; truncation repairs the list. Review/config changes also
invalidate affected selectors; settings need a declared project settings-change
signal. Reuse subscribe/coverage repair, push fencing and cross-tab read-order
infrastructure [E9]. For this subfamily, hold subscription coverage before the
snapshot or re-read when coverage is late; per-record version fences prevent a
slow pull overwriting new evidence. Reconnect refetches bounded current data;
no durable generic replay log is promised.

Baseline wire is 3.4.0. Allocate the next available additive minor **at
implementation**, not a hard-coded future number. Update `api/mod.rs`,
`fixtures/api/versions.json`, `session.hello.json`, each verb fixture's `since`,
`events.json`, and the prior-minor verb/capability manifest using
`scripts/api-verbs-manifest.mjs` [E1]. Advertise method names and a complete
`tasks.localReview` capability only when the usable vertical slice is ready;
optional params on existing verbs need their own named feature. New SPA on old
bridge keeps normal task/Changes surfaces and explains the missing review
capability. Cache any flag that changes paint; dispatch on the current greeting
via `whenGreeted` [E9]. Never send safety fields to a legacy merge verb and hope
it implements them. Retiring/changing old semantics requires separate versioning.

RPC/MCP handlers perform typed conversion, authenticated scope checks and invoke
services. Proposed isolated `task_reviews` service owns transactions, admitted
Git operations, workflow handoffs and recovery while clients are absent. SPA
owns selectors, workflow editing, reviewer selection and action composition;
settings supply reviewer/check policy. Do not introduce a general downloadable
policy runtime or move an always-on loop into a browser [E11].

## 3. Git mechanics

### Local publication and target authority

The workspace branch is the source. A task review publishes references to commits, not
a copied patch and not uncommitted/index state. Worktree sources share objects
with their parent repository; rift sources have independent refs/object stores.
For both, capture exact `H` from the registered source checkout, import/pin it
in the canonical project source repository, and verify the imported commit.
For rifts use a local fetch like `git fetch --no-tags <registered clone>
refs/heads/<branch>:refs/build/task-reviews/<task>/<attachment>/<revision>/head`, with a fresh private
ref and no force prefix, then verify its OID equals `expected_head`. Reject a
race and do not publish the revision on mismatch. Do not call the current
`publish` helper: it force-updates shared `refs/heads/<branch>` and another
clone may use that name. An external push is unrelated to updating the task review.
Reads always use the private retained refs, never a workspace's mutable HEAD.
Publication is explicit; phase 1 does not depend on clone/common-ref filesystem
watchers. Refresh/reopen/prepare re-read live facts before an action.

The target is an explicitly registered local repository/ref, defaulting to the
source's project repository and configured base branch (usually `main`). Never
infer `origin/main` or a same-named branch in an arbitrary integration clone.
An existing integration clone can be selected as authority only through an
explicit binding; the UI names where the merge will land. Additional source
directories need this mapping before they can open task reviews. Detached HEAD,
non-Git directories, unrelated histories, source equal to target and unresolved
repository identity are precise refusals.
When authority is a separate registered local clone, capture/import its exact
target `T` into private refs before preparing in the canonical source repo, then
import and verify exact candidate `C` in the authority before landing. Query
target ancestry/coverage there. Object transfer is local in both directions and
never changes a shared source branch or relies on a network remote.

### Candidate, checks and merge

Recommend a **no-fast-forward merge commit**. It preserves reviewed commits and
anchors and gives each task review a visible integration boundary; it also matches
the integration convention recorded in #89's task brief. Squash would require mapping the reviewed
history to another commit; rebase rewrites it and complicates approval and
recovery. Neither is offered in phase 1. Even a fast-forwardable source gets a
merge commit. A source already contained in target returns `already_integrated`
with evidence and asks the controller to reconcile; it does not create a second
merge or falsely attribute an external merge to this operation.

1. **Prepare.** Resolve/pin exact `H/T/B`, take a per-repository operation lease,
   and run `git merge-tree --write-tree -z --name-only --messages T H` in the
   canonical repository. Exit 1 with a valid conflict result records conflict
   paths; other failures are errors, not mergeability answers. On success use
   `git commit-tree <tree> -p T -p H -F <message-file>` to create `C`; record its
   tree, author/message and SHA, and pin it. Include task/round/revision trailers in the
   message. Reuse the persisted candidate for the same parents and merge inputs,
   including metadata; do not recreate it with a new timestamp on every read.
   Probe the installed Git for the needed merge-tree/commit-tree behavior and
   flags before advertising preparation; refuse unsupported Git explicitly.
   The implementation must cover these command forms in temporary-repo tests;
   this plan has not executed them against a live project.
   Merge conflicts leave the target unchanged.
   The author resolves on the source branch, commits and publishes a new
   revision; Build never silently edits the source to resolve a conflict.
2. **Check.** The project agent uses a checkout it controls, such as its existing
   integration scratch checkout or a workspace, locally fetches the retained
   candidate ref and checks out exact `C` detached there. This is separate from
   the target checkout; no new workspace per check is required. It records cwd and verifies HEAD
   and tracked cleanliness before/after running configured gates with existing
   execution, capturing actual exit status. Record divergence explicitly; a run
   against edited tracked files is not clean-candidate evidence. Build's
   initial set is SPA lint and full Vitest, gitleaks, semgrep, and diff-check,
   plus applicable bridge tests for bridge changes. Command definitions and
   requiredness are project policy; record exact commands and versions, not a
   hard-coded Rust list. Record runs against `C` with raw exit/signal, log and
   timestamps. A missing, interrupted or skipped check is not success. A rerun
   appends evidence instead of overwriting failure. Phase 1 clearly labels this
   evidence actor-attested; a generic process scheduler is not a prerequisite.
3. **Decide.** SPA/project agent evaluates current review submissions and checks.
   It supplies evidence IDs, expected review/configuration versions and exact candidate
   tuple. New review decisions or revision changes invalidate an old admission.
   Target advancement needs a new candidate and checks; approval remains bound
   to unchanged source revision `H`. Checks of `H` alone do not cover `C`.
   Persist the selected check requirements/version with the workflow. Admission
   matches selected successful execution IDs against exact C and that recorded
   requirement set; no missing run or caller-supplied success boolean counts.
   Names and commands are project inputs, not a fixed Rust list.
4. **Apply.** Persist a write-ahead operation containing `H/T/C` before target
   mutation. Revalidate qualifying approval, lifecycle/versions, object
   connectivity, target identity, target SHA and candidate tree. Advance the named target from `T` to
   exact `C`, never recompute a different merge after checks. Serialize competing
   Build operations using one repository/ref reservation shared with source sync,
   review checkout, direct git writers and reclaim (new integration work [E5, E6]); preserve the caller's chosen merge/evidence in the receipt.
5. **Settle.** Confirm target ancestry and checkout state, record this attachment's
   merge receipt and task event transactionally, then emit the task invalidation.
   The round displays Merged only after all its required attachments integrate.
   Network pushing is a separate caller step and, for Build's main branch, a
   deployment action gated by coverage. Task completion, deployment and
   cleanup are separate explicit steps and may fail separately.

**Land in the actual target checkout.** Enumerate `git worktree list --porcelain`
in the authoritative repository. If target is not checked out anywhere, use
`git update-ref <ref> C T`, a true ref compare-and-swap. If checked out at path
`K`, require HEAD on the target at `T`, no tracked dirt/in-progress Git state,
and no active Build writer whose cwd is that checkout. This is checkout-scoped,
not project-scoped: the project agent runs from its own scratch directory and
must be able to call merge. Then run `git -C K merge --ff-only --no-overwrite-ignore C`, allowing Git's
untracked-overwrite checks to refuse. Never stash/reset a user's work. If the
target is checked out more than once, refuse the ambiguous arrangement.

Source sync already uses unattended Git and disables hooks/automatic
maintenance [E6]. Reuse those process/timeout primitives for prepare/import/land,
with structured argv and no arbitrary external diff/textconv. Do not copy its
mutable tracking-ref operand: landing must name retained C. Keep filters needed
for a correct checkout, with bounded execution and explicit failures.

Run prepare/land Git with hooks disabled for deterministic primitive behavior;
post-merge and reference-transaction hooks do not run for these actions. This
visible choice is listed for Zech below. Explicit gates remain separate.
Use full refs/OIDs and structured arguments; no branch/Markdown interpolation
into shell. Refuse incomplete/submodule-specific checkout states that cannot
be handled safely in phase 1.

The checked-out path is a guarded fast-forward, **not** an atomic CAS across
Git, index and files. Serialize Build writers and use Git's locks/overwrite
checks; reread actual target/index state afterwards. An unrelated external
advance normally refuses ff-only; if target already contains `C`, reconcile
the recorded candidate rather than claim a second landing. External file writes
remain outside Build's lock. An interrupted update can leave target at `T` with
a dirty checkout: record interrupted/uncertain and require inspection. No
automatic checkout repair. Test these boundaries explicitly.

Fixed `H/C` protects what is merged even if an external Git process subsequently
moves the source. Recheck observed source state before admission and report a
known change, but do not claim to lock arbitrary external Git/file writers with
a Build mutex. Target locking and checkout ownership are execution requirements.

### Recovery and cleanup

Journal phases include `accepted`, `prepared`, `applying`, `ref_applied`,
`completed`, `failed`, `uncertain`. On restart inspect Git before retrying:

| Observed state | Recovery |
| --- | --- |
| Target still `T`, no target effect recorded | Reconcile candidate/checkouts and expose retry under the same operation ID; never rerun checks as an invisible side effect. |
| Target exactly `C` or a descendant containing retained `C` | Verify recorded parents/tree, repair DB completion once, retain original merge SHA. |
| Only current revision `H` is reachable, and recorded `C` is not | Record external integration with observed target SHA; original merge SHA/check provenance unknown. Do not declare our merge operation successful or grant deploy coverage. |
| Target unrelated/moved, or target checkout inconsistent | Mark uncertain/conflict, expose actual facts; no reset or blind replay. |
| Git success, task update/roll/cleanup fails | The attachment stays integrated; reconcile its receipt once and retry only the separate failed effect. |

No cross-store atomicity is claimed between SQLite and Git. The retained `C`
and journal make reconciliation possible. Test termination at every boundary,
including after ref change but before database commit and after commit before
response. A timeout is a reason to query `tasks.review.operation`, not merge again.

Keep the workspace, source branch and task history after merge. Follow-on work
requires an explicit new task or reopened task/new round; it never amends the
already merged round. Cleanup checks every workspace directory, agents and
terminals anywhere beneath it, dirty/untracked files, and reachability of all
current work. Retained review pins preserve history, not every unpublished
follow-on commit. Never delete adopted/user checkouts.

Current `workspace_finish_legacy` honors branch deletion, and merged-run Done
can close linked tasks [E4]. Current reclaim/path containment and branch
restoration guards are substantial existing code [E5]. Reuse those mechanisms
with a new explicit local-integration proof; do not bolt on another destructive
workflow or route through the merged-Done close hook. Keep legacy remote-based
finish/reclaim behavior unchanged for callers without the new capability.

`workspace.cleanup_integrated` measures each directory's **current checkout
HEAD**, imports that exact object if needed, and proves reachability from its
registered current target. Never substitute last reviewed H or a same-name
branch in the canonical repository for a Rift's live head. Refuse plain
directories, dirt, untracked work, missing objects or active writers. Under the
shared reservation, recheck heads/status before deferred removal, preserve
managed-root/descriptor boundaries, and journal partial removal. Branch deletion
needs an explicit local proof and expected-tip CAS, default/checked-out branch
guards and restoration reporting; current remote-only deletion proof is not
reusable unchanged. A pin by itself never authorizes deletion. Task closure
and workspace reclamation remain separate even after successful deployment.

### Deployment coverage

Build's current CI deploys on pushes to main and manual workflow dispatch
(`.github/workflows/ci.yml`, [E12]). The review data and operations remain local, but
Build's own rollout procedure must check review coverage **before a main push,
CI dispatch, or local bridge/app restart that deploys new work**, not afterwards.
`tasks.review.coverage` walks first-parent commits between an explicitly trusted last
deployed SHA and the exact proposed release SHA. Require the baseline to be an
ancestor; page with explicit completeness, never treat the first page as all.
Each integration commit must match a retained completed attachment candidate
with its qualifying task-round approval evidence. Direct commits, `H`-only external integrations,
missing receipts and unqualified approvals are uncovered and block the normal
roll procedure. Each candidate binds its attachment’s source-side work; earlier
target integrations are covered independently along first-parent history.

At deployment re-evaluate retained submissions against current project config
and latest decisions, in addition to the evidence snapshot at merge. A removed
reviewer model or withdrawn approval can therefore block a still-pending roll.
`tasks.review.submit` remains available on merged task reviews: an approver's later withdrawal
or request-changes means that approval no longer qualifies for deployment.
Another reviewer's change request does not veto qualifying evidence. A new
qualifying approval of that unchanged revision or a user retrospective range
approval restores coverage; history and the completed merge stay unchanged.
Return commit/task/round/evidence/config versions and reasons, never accept a caller's
`reviewed: true`. The project agent verifies the exact to-SHA and complete
coverage immediately before the deployment action and records the result; a
new main commit requires another check. Show uncovered commits in the SPA.

This does not make CI consult a local database or prevent a same-user shell
from pushing anyway. Implement the check in the project-agent roll procedure
and any Build-owned deployment entry point; manual shell bypass remains a
workflow violation. Strong prevention would require a separately trusted deploy
gate/CI evidence transport, outside this local task review scope. Task trailers identify
work but are not approval evidence. Initial adoption needs a user-selected,
verified deployed baseline for each deployed tier (app, relay, bridge may differ); it does not retroactively certify older history.
Include retrospective user review in phase 1, subject to Zech's product answer
below: show the exact from/to diff, then record `tasks.review.approve_range` with the
Git-verified to-tree. Coverage accepts only that repository and explicit
first-parent range; descendants outside it still need evidence. The baseline
must lie on the first-parent chain, not merely be a reachable side-parent.
Range endpoints, tree, actor and approval ID are immutable; user-only withdrawal
removes that evidence. No agent can manufacture this user approval through MCP. Range review uses
the same task diff surface and timeline, with explicit immutable from/to scope.
Coverage must use the release SHA each deployment actually consumes; checking
a mutable main and then dispatching a different SHA is not sufficient. If a
legacy deployment trigger cannot pin that identity, refuse the managed action
or add that precondition before claiming it is covered.
This makes direct user commits/external merges reviewable before deployment
instead of wedging the rollout. There is no silent retrospective exemption.


## 4. SPA

Use the existing tracker task route, canonically
`#/device/<deviceId>/project/<projectId>/tasks/<taskId>` (device-less links resolve
to it), and add a Changes/review section with selected attachment and revision
in route/query state. Preserve the existing `/c/<commentId>` focus when composing
those selections. A filtered review queue
is a view of Tasks, not a new project PR collection. Keep the project rail and
shell; workspace Changes links to the owning task or explicitly chooses/creates
a task before attaching its selected directory. Current routes and renderers
are [E8]; proposed routing extensions need parser/builder and shell tests.

The task page shows its title/body, single timeline and assignee alongside:

- All attached repository branches, source/target identity, current round and
  revision, and whether the live source has moved. Non-Git/unborn sources show
  why no committed review is available.
- Commits, changed files, lazy diffs, old/new line and hunk feedback, replies,
  resolution and outdated-thread links. A timeline comment links to its exact
  immutable diff; a diff comment links back to that same timeline record.
- Per-reviewer decision/model evidence, qualification reason, current workflow
  step/recipient, gate execution evidence and partial/all integration state.
- Explicit publish/open, request review, approve/request changes, prepare/check,
  merge and step advance actions. Drafts, submissions, merges and deployment
  are different actions; no “Complete implies approve” shortcut.

Approval applies to the visible complete revision set. Make missing bodies,
truncation and unresolved feedback visible. Display config changes and stale
review/candidate checks. On narrow screens these are sections of one task page,
with a persistent shell and no horizontal overflow caused by filenames/actions.
The human step names the decision and destination: approve/advance or send back
with comments. Agent steps do not manufacture a Needs you pause.

Reuse the rendering, viewport, folds, lazy bodies, place keeping, escaping and
composer freeze behavior of `changesReview.js`, `changesComments.js`,
`changesetBodies.js` and the diff stack [E8]. Extract a focused adapter that
reads task anchors and writes task comments; do not inflate the legacy review
controller with workflow policy. Current `run.request_changes` posts into a
conversation, and file viewed marks are local UI state [E8]. Keep that adapter
for adopted/legacy review; neither constitutes approval of a tracker task.
`taskReview.js` is the old plan surface, not the new task review controller.

Add `taskReviewCache.js`, `taskReviewSync.js`, `taskReviewModel.js` and small
mutation/thread adapters beside the existing tracker modules. Key current-review
replicas by device + project + task; keep aggregate version in the record and
write fence, not its address. Key immutable bodies by device/project + repository,
with revision/blob/path/options. Task-owned history must not be evicted when its workspace
is removed. Reuse `bodyPages.js`/`cachedBodies.js` continuation and content-key
fences; body metadata and pages commit before painting [E9]. Keep absent, empty,
truncated and unavailable distinct. No fresh uncached-Git exception is needed.

Every path is `RPC/push → versioned cache write → notification → cache read →
paint`. Mount the cache-backed task page using the route's `surfaceContext`;
the page paints held records on cold/offline loads before refreshing. Never
paint a mutation reply directly. Preserve drafts
in `localUiState`; optimistic comments are pending task records reconciled by
stable operation ID, while approval/merge stays pending until admitted. Reuse
existing cache recovery rather than adding a private memory store [E9].

Current tracker list paging already has cross-tab read ordering [E9]. Add
review summary/version participation to that fold; do not replace it with a
separate list cache that resurrects old membership. A versioned current-review
read supplements the task timeline. Settings and capability changes write cache
records before selectors/button paint change; operation dispatch still checks
the live greeting/bridge preconditions.

## 5. Agents and workspace handoffs

Keep `get_task`, `comment_task`, `assign_task`, `move_task`, `link_task` and
existing workspace/agent creation tools [E7]. Add a small task-scoped tool
family mirroring the new contracts: `open_task_review`,
`publish_task_review`, `get_task_review`, `read_task_review_diff`,
`read_task_review_file`, `submit_task_review`, `handoff_task_review`,
`advance_task_review`, `prepare_task_merge`, `record_task_check`,
`merge_task_branch`, `get_task_review_operation`, `check_deploy_coverage`.
Use `comment_task` for feedback/replies with optional anchors. Do not add a
parallel `comment_review` stream or a `list_reviews` object inventory.

MCP derives project and actor from the bound conversation, validates scope on
every child ID/repository/blob, and enforces action surfaces at dispatch as well
as discovery. Prepare/merge/cleanup default to Project surface; authors/reviewers
use Coding surface for publication/findings. A deploy agent in a workspace needs
an explicit scoped capability/dispatch entitlement before these operations
become available there, not a misleading prompt telling it to call tools its
surface cannot use. First release may use the project agent as the deploy step;
independent workspace deployers require that entitlement in the same slice.
User config, human-step approval and retrospective range approval have no MCP
write counterpart. Reuse current model discovery and role choice; the service
records actual session evidence, not a reviewer-supplied model string [E7].

### Branch movement is part of the handoff

1. The implementer commits and publishes a task round with exact attachment
   revisions. “Ready to roll” becomes a review handoff on the task.
2. The controller/user chooses the next explicit agent or human step. For an
   agent reviewer, persist a handoff intent naming task/round, old assignee,
   destination workspace and recipient; reserve destination checkouts before
   materialization. The old assignee is not silently stopped. Any ongoing writer
   at the destination blocks the checkout step.
3. Prepare the reviewer's own managed workspace via existing provisioning
   machinery, importing locally from retained refs and checking out exact H.
   Review-only checkouts may be detached; editable handback uses an explicitly
   owned local branch. A worktree cannot simultaneously own a branch already
   checked out elsewhere; use a separate review branch/detached checkout or a
   Rift clone. Never run ad hoc `git worktree add` on a registered repository.
   Existing `workspace.create` normally cuts new branches [E4]; attaching an
   existing immutable revision is **new work**, not current assignment behavior.
4. Verify every destination's repository/HEAD, record bindings, then settle the
   task assignment and durable reviewer delivery. Existing tracker dispatch
   already prepares assignments and rechecks assignee state around deferred work
   [E2]; extend that service rather than call raw assignment twice. The review
   intent, step generation and operation ID make retries/readback unambiguous.
5. Delivery carries task ID, exact round/attachments/H/T/B, scope, expected
   outcome and instructions to read the task/current review. Reviewer comments
   enter the task timeline and reach the assignee like other task comments.
   One decision is submitted for that round. A late decision remains historical.
6. Changes requested applies the configured backward transition, assigning the
   task to its saved implementer (and preserving reviewer tracking). Fixes may
   happen in another registered workspace; transferring writable ownership uses
   expected source head/step generation and no force overwrite. New commits are
   imported into fresh private revision refs, then explicitly re-reviewed.
7. The current step's designated reviewer explicitly submits and advances its
   step; another observer's comment/decision never redirects the workflow. A
   late step/round generation cannot advance it. A later human step, if present, waits for
   that user. Otherwise the task goes directly to its configured deploy agent,
   which prepares/checks/merges exact candidates, checks release coverage,
   deploys and records verification before task completion.

The task owns a **small persisted sequence**, sufficient for implement →
review → deploy, including explicit backward edges and human/agent recipients.
A general workflow editor, arbitrary loops, parallel DAGs and cross-device
checkout transfer are follow-ons under #145. The first slice must still make
manual handoff between workspaces truthful and durable; a mere message to a
reviewer on the implementer's moving checkout does not satisfy #145.

Separate the service from RPC/event delivery [E11]. A bridge restart resumes
accepted handoffs after checking workspace/HEAD/assignee facts; it does not
repeat checkout or launch blindly. Store intent and delivery receipt together,
then reuse operation-ledger and delivery-runner claim/uncertain handling [E3,
E7]. A DB transaction cannot include provider delivery or filesystem creation;
journal those boundaries. A crash after checkout but before assignment can
leave an idle reusable workspace; it must not start an unassigned reviewer.
Concurrent reassignments fail expected-generation checks and never overwrite a
human's newer assignment. Two browser tabs cannot become two schedulers.
Today `assign_task_to`/`hand_over` can accept delivery before persisting the
settled assignment [E2]. Do not treat a call to that existing method as an
atomic workflow handoff. Extract its preparation/delivery pieces, persist the
step/assignment and queued delivery intent together, then dispatch from that
durable intent; retain the legacy ordering for non-opted-in callers until
separately migrated. Fault-inject task-write failure after preparation and
restart before/after delivery claim.

Ordinary task notices retain the existing fan-out/coalescing path; don't claim
it is already a transactional outbox. Explicit workflow handoffs and decision
transitions require durable delivery intent added to the store. Agent messages
say to read the task; they are not a second authoritative feedback record.
A generic Complete report or idle session never advances this workflow. Report
and transition outcomes are separate, both visible on the task when relevant.

Update `bridge/templates/notes/task_tools.md`,
`bridge/templates/notes/workspace.md`, `bridge/templates/project_agent.md` as
applicable, their assembly in `bridge/src/templates.rs`, MCP descriptions and
`bridge/src/orchestrator/workspace.rs` together [E7]. Teach:

- Read task and exact active round before acting; publish only committed work.
- Findings/replies belong on that task with immutable anchors. No acknowledgments,
  starting notices or repeated diff recaps; post decisions, blockers, questions,
  changed state and necessary evidence.
- Submit approval/request-changes explicitly; Complete is not a review or gate.
- Handoff assigns the task and names its input revision/destination. A human is
  involved only where the workflow has a human step or explicitly asks them.
- Before deployment/main publication, verify complete review coverage for the
  exact release SHA; shell access is not permission to bypass the rule.

Keep new instructions capability-gated. Current templates are compiled defaults,
with no per-project override support (`templates.rs:3–8`); do not claim an
app-only prompt update or invent an override dependency. Do not ask legacy
agents to call unavailable tools. This revision
uses always-on bridge services per #86, not the old plan's proposed browser-owned
prompt runtime.

## 6. Tasks integration (the original Issues integration section)

The task page is the review page. Ordinary branch/workspace/commit links remain
for navigation, while typed attachments establish exact repository authority.
Related tasks use normal links/parent references; they do not gain duplicate
comments, approvals or completion by being mentioned. All review events,
requests and gate/merge outcomes share the task timeline.

Current `app/tracker/activity.rs` moves a held dispatched task to In review on
Complete; merged workspace Done can close linked open tasks [E2, E4]. These
hooks must be narrowed for **explicitly opted-in workflow tasks** in phase 1.
Persist workflow ownership/generation in the dispatch context, and make these
legacy hooks yield to the review service for that task. Existing non-review
work retains its old behavior. Prompts alone cannot stop automatic hooks.

For review workflows, In review means an explicit human step needs Zech.
Agent-only review stays In progress with its reviewer as assignee. A human
step assigns the user and sets In review; Needs you derives through existing
assignment/mention/read-mark rules [E2]. Moving back to an agent restores the
appropriate active column via the explicit transition. Simply opening a diff,
receiving a comment or entering an agent step never requests human attention.

“Done with this step” is not the board's Done column. The review step can
finish and dispatch deployment while the task remains In progress. Recommended
completion for Build: all required attachments integrated, exact release
coverage checked, deployment verified, then an idempotent task Done event and
explicit closure. For projects without deployment, configure an end-after-merge
step. Closing remains separate from the column; #145's last “issue is closed”
is an explicit terminal workflow action, not a side effect of deleting a folder.

Completion records the required attachment set and evidence versions. Partial
merge/deploy failure keeps the task open; already integrated branches stay
integrated, and retries only address remaining effects. A failed task update
cannot roll back Git. User closure/cancellation pauses further workflow
admission, preserves historical decisions and branches, and does not pretend to
undo effects already accepted. Reopening requires an explicit new active round
or resumed step, never silent replay of an old deployment.

The planning task #89 itself still goes to In review for Zech's read when its
deliverable is reported Complete. This plan does not change current task
behavior or opt itself into an unimplemented workflow.

## 7. Rollout

Ship one usable vertical slice behind `tasks.localReview`: task branch
attachments → durable comments → independent review in another workspace →
fix/re-review → candidate gates → safe local integration → explicit completion.
Include the narrow sequence/human-step semantics needed by #145; defer its rich
workflow editor. No product code or roll occurs in this planning task.

Sizes are estimates including review and failure-path tests: S = 1–2 focused
engineer days, M = 3–5, L = 6–10. The original 24–40-day first-release estimate
predated task ownership and cross-workspace workflow work. Revised estimate:
**30–48 engineer days** for the complete first slice, with overlap possible
after the contract is agreed. The baseline has more reusable infrastructure,
but durable handoff/model-evidence integration remains substantial work.

| Phase | Concrete files / deliverable | Tests and exit condition | Size; rollout |
| --- | --- | --- | --- |
| 1a: task records and contracts | New `bridge/src/task_reviews/` domain/service, `store/task_reviews.rs`, `app/task_reviews/`, `api/v1/task_reviews.rs`; extend `tracker.rs`, `store/{schema,tracker,operations}.rs`, module roots, `app/rpc.rs`, task typed DTOs; fixtures and version manifest. Branch bindings/rounds/anchors, task comment extension, current projection, operation IDs, config/user-only range review. | Additive migration/restart/backup; same ID different body; project/actor/path fences; aggregate versions and legacy task shape; current data independent of paged timeline; pin-before-publish recovery; both contract suites. | L, 6–10 days; bridge, feature not announced. |
| 1b: Git and lifecycle | New task-review git/candidate/recovery modules; reuse `app/git/deferred.rs`, `git_process.rs`, isolation/worktree APIs; integrate `source_sync.rs`/`app/projects/base_sync.rs`, `app/workspaces/{reclaim,deletion,branch_delete}` and containment guards. | Real temporary-repo tests for local worktree/Rift imports, name collision, no remote, GC after workspace deletion, exact candidate, source/target moves, sync/reclaim races, dirty/ignored/in-progress/multiply checked-out target, hooks/filter failure, crash boundaries and uncertain checkout. Local cleanup tests live heads/all directories/unpublished fixes. | L+, 8–12 days; bridge, hidden. |
| 1c: task review UI/cache | New focused `spa/src/core/taskReview{Cache,Sync,Model,Actions,Threads}.js`; extend `spa/src/core/{trackerTaskPage,trackerTaskRender,trackerTimeline,trackerCache,cacheSync,router,shell,changesReview}.js` and `spa/src/views/trackerTaskView.js`; reuse body/draft stores and styles. | Cached cold/offline mount, actual cache-write-to-redraw, cross-tab older page/new decision, late subscription/reconnect/truncation, same comment in diff/timeline, rename/deletion/hunk/outdated anchors, body continuation, draft preservation, 390px layout and keyboard review. | L, 6–10 days; app plus 1a/1b bridge capability. |
| 1d: handoff and admission | New isolated task-review workflow/admission service; extend `app/tracker/{dispatch,activity,notices,tools}.rs`, `delivery.rs`, `store/operations.rs`, model/session observation capture, `mcp.rs`, `app/mcp.rs`, templates/notes and `orchestrator/workspace.rs`; workspace exact-revision provisioning. | Separate reviewer workspace; existing branch collision; human versus agent step; implementer self-approval rejection; missing/fallback/mismatched observed model; concurrent reassignment; uncertain delivery; duplicate restart; old Complete/finish hooks bypassed only for opted-in tasks; no browser for entire flow. | L+, 8–12 days; coordinated bridge + app activation. |
| 1e: release integration/acceptance | Project-agent instructions/roll procedure and any Build-owned deploy caller, range-review UI, capability activation, evidence for security checklist (new review checklist during implementation). Amend `ARCHITECTURE.md`, Tasks/Wire specs to implementation then. | Candidate checks pinned to C; coverage by exact deployed tier baseline/SHA; direct/external work blocked until explicit range review; withdrawal/config changes; failed deployment doesn't complete task; no remote end-to-end through restart/history/cleanup; old/new app/bridge matrix. | S–M, 2–4 days; procedure + app; bridge only if contracts changed. |
| 2: task workflow presentation | Review queue/filter polish, comparison selector, workflow editor over already shipped step primitives, viewed marks and task navigation. | Cache/route/mobile and persisted explicit-step consistency. | M per feature; app-only when semantics fit shipped contracts. |
| 3: additional device capabilities | Stronger device-observed gate execution, safe retention/rebinding, optional anchor mapping persistence or cross-device handoff (separately designed). | Provenance/retention/failure/ambiguity-specific tests. | M–L per capability; bridge + app when device contracts change. |

Contract design and review precede splitting implementation. Do not activate a
record-only feature that cannot complete the workflow. No resurrection of the
removed plan scheduler, independent PR collection, automatic merge queue,
GitHub sync, squash/rebase choices, stacked same-target attachments or atomic
multi-repository landing in phase 1. Keep immutable anchors, recovery and truthful
evidence rather than trimming correctness to fit a roll deadline.

Follow AGENTS.md's TDD and gates for implementation: relevant Rust tests,
format/clippy, SPA lint/full Vitest/build where applicable, gitleaks before every
push, semgrep before ready, and diff-check, all under `nice -n 10` where
applicable and judged by exit status. Add security checklist coverage for
scope/identity, model provenance, anchors, process inputs, lifecycle boundaries
and durable effects; this plan does not claim an unimplemented checklist passes.
Use disposable test repositories, not user checkouts, bridge binaries or an
actual deployment to validate the proposal. The plan-only deliverable is checked
for code-reference accuracy, eight-section coverage, stale-decision consistency,
whitespace and required scans; app suites cannot validate Markdown behavior.

## 8. Open questions for Zech

These are product choices with recommended defaults, not permission to skip
review or missing prerequisites to complete this draft. Record answers before
activating their behavior. #145 already settled task identity, one discussion
stream, reassignment, and explicit human step types; those are **not** open.
The qualifying independent-review requirement from #89 is likewise settled.

| Question | Recommended answer / consequence |
| --- | --- |
| What should the task surface call this? | **Changes** on the task, with a **Review** state/action and a review filter in Tasks. No separate PR terminology or numbering. |
| How much #145 workflow ships first? | A narrow persisted implement → review → deploy sequence with explicit human/agent steps and manual configuration; rich editing and parallel graphs follow. Cross-workspace checkout and handback are required initially. |
| Which merge strategy? | No-ff merge commits only, retaining original commits and an exact candidate boundary. |
| Can one task review multiple repositories? | Yes, one required attachment per repository/target initially, one immutable round covering all; show partial integration and do not promise atomic merge/deploy. |
| What happens after merge? | Keep workspaces/branches/history. Explicit cleanup uses current-head local integration proof and existing lifecycle guards; no automatic deletion. |
| When is the task Done/closed? | For Build after verified deployment; without deployment after verified merge. Terminal workflow explicitly moves Done and closes; it never closes related tasks by workspace membership. |
| How conservative is stale approval? | New H, retargeting or attachment-set change requires review; target-only advance requires new C/checks. One qualifying approval suffices; disagreement stays visible. |
| Are agent-reported gate results enough initially? | Yes, clearly attributed and pinned to exact C with cwd/cleanliness/exit evidence. Do not label them bridge-observed. |
| Which merge target checkout is supported first? | Explicit registered authority; CAS if target isn't checked out, otherwise guarded ff-only to retained C in its actual clean checkout. Coordinate source sync; refuse dirty/busy/ambiguous targets. |
| Disable hooks during preparation/import/landing? | Yes, consistent with source sync; configured validation runs as explicit gates. Retain necessary checkout filters and report failures. |
| Retrospective review for already-integrated work? | User-only exact-range approval on a task in phase 1, with explicit per-tier deployed baseline. No inferred approval from trailers/reachability. |
| Can the reviewer also merge/deploy? | Yes when independent of implementation and qualified; a workspace deployer needs explicit scope. No third actor is mandated. |
| Which exact model IDs qualify initially? | User saves the current qualifying models from discovery. Preserve the Fable/Astra-class requirement without silently treating the old sample IDs, model aliases or catalog fallback as runtime evidence. |

## Workshop record and current code evidence

**Historical round 1 (2026-09-22):** Opus proposed checkout-free candidate
construction, actual-target ff-only landing, SHA-indexed checks, exact-blob
anchors, reused notices and local cleanup; Astra accepted. Astra retained draft,
a small merge journal and explicit completion. Private-ref Rift import replaced
force-publication. A second bare archive/general replay framework was removed.
Independent PR numbering was agreed then and is now superseded by #145.

**Historical round 2 (2026-09-22):** mandatory qualified review replaced optional
review. User-only config, session/model evidence, persistent implementer
identities, shell-bypass limits, release coverage and retrospective review were
added. Opus agreed after clarifying local publication, existing gate checkouts,
range review and withdrawals. This is preserved history, not approval of the
current revision.

**Refresh draft (2026-10-02):** the original commit was reapplied unchanged onto
today's main before editing, preserving a direct diff against the prior plan. Main advanced
during the audit from `99e659a3` to `adf08b06`; the intervening #321 changes
were inspected (pairing/presence, plus architecture and app wiring), and this
branch was rebased onto the latter without changing the review conclusions.
#89 and #145 full timelines and all five historical #86 audit sections plus its
later user correction were read. This document now folds review into tasks,
corrects stale wire/cache/lifecycle assumptions, adds explicit workspace
handoff/step recovery, and records source-sync coordination. The project agent
will arrange the next Opus challenge against the pushed SHA. Record its findings,
lead decisions and a revised summary on #89; do not carry over the prior
“no blockers” statement as fresh signoff.

The evidence index records current code, not proposed module existence. All
paths are repository-relative at the baseline SHA above. The listed existing
symbols are the implementation seams; new files in section 7 are proposals.

| Ref | Current source and fact |
| --- | --- |
| E1 | `bridge/src/api/mod.rs:32` (`API_VERSION = 3.4.0`), `fixtures/api/versions.json`; `api/v1/mod.rs` (`parse_params`, typed dispatch); `api/v1/tasks.rs` (tracker verbs); `scripts/api-verbs-manifest.mjs`, `bridge/tests/api_contract.rs`, `spa/test/apiContract.test.js`. Wire names/strictness/capability/version discipline; `api/v1/` is not the wire major. |
| E2 | `bridge/src/tracker.rs:213,275,477` (TaskLinks, Task, TaskComment); `bridge/src/app/tracker/{mod,views,pages,dispatch,activity,notices,tools,attachments}.rs`; `bridge/src/store/tracker.rs:144` (`save_tracker_task_activity`). Existing canonical task/timeline, path-scoped storage, uploaded attachments, paged list, assignment/notice and Complete behavior. |
| E3 | `bridge/src/store.rs` (Store/open), `bridge/src/store/schema.rs`, `bridge/src/store/operations.rs:26` (`backup_to`); `bridge/src/app/conversations/operation_ledger.rs`, `bridge/src/app/runtime/delivery/receipts.rs`. SQLite authority and operation receipts exist; no current task-review transaction is implied. |
| E4 | `bridge/src/app/workspaces/mod.rs:544` (`workspace_create`), `:648` (`finish_workspace`), `:808` (`workspace_finish_legacy`); `app/workspaces/deletion.rs:314` (`remove_workspace`, closure only for merged Finish); `workspace.rs:1004` (`summary_finish_blockers`). Current creation, Finish deletion support and remote-publication completion guards. |
| E5 | `bridge/src/app/workspaces/reclaim.rs` (reservations), `app/workspaces/reclaim/explicit.rs`, `reclaim/containment.rs`, `app/workspaces/branch_delete/{mod,checkouts,defaults}.rs`; `gitgui/unpushed.rs`. Managed-path safety, explicit reclaim and conditional branch deletion already exist; their publication proof is remote-based. |
| E6 | `bridge/src/app/projects/mod.rs:8` (`ProjectSource`), `:25`/`:29` (sync defaults), `app/projects/base_sync.rs`; `source_sync.rs:309` (`fast_forward`), `:424` (`SyncLock::acquire`), `source_sync/{checkout,in_progress,git}.rs`. Background/base-at-cut synchronization is a competing writer; current locks are sync-path-specific. `isolation/rift.rs:396,509` force-fetches shared branch refs; `isolation/worktree.rs:58` needs no publication because objects are shared. |
| E7 | `bridge/src/mcp.rs` (`BridgeAction`, tool lists/surfaces); `bridge/src/app/{mcp,tracker/tools,runtime/spawning}.rs`; `bridge/src/{delivery,agent,templates}.rs`; `bridge/src/app/runtime/agents/{records,endpoints}.rs` (observed model versus digest fallback), `bridge/src/app/runtime/sessions/registry.rs` (generation), `bridge/src/harness/installed/`; `bridge/templates/notes/{task_tools,workspace}.md`, `bridge/templates/project_agent.md`, `bridge/src/orchestrator/workspace.rs`. MCP authority, assignment delivery, model/prompt seams. |
| E8 | `spa/src/core/router.js:145,492`, `spa/src/app.js:296`, `spa/src/views/trackerTaskView.js`, `spa/src/core/{trackerTaskPage,trackerTaskRender,trackerTimeline}.js`; `spa/src/views/{taskReview,worktreeReview,workspaceChanges}.js`, `spa/src/core/{changesReview,changesComments,changesetBodies,diffRender}.js`; `bridge/src/thread/items.rs` (`ThreadLink`), `bridge/src/app/tracker/refs.rs`. Tracker versus legacy review routes; existing diff/composer stack and mutable file refs. |
| E9 | `spa/src/core/{localCache,localUiState,localUiStore,cacheSync,pushFence,taskReadOrder,bodyPages,cachedBodies,surfaceContext,deviceContexts}.js`; `core/bridgeApi/{index,v1/index}.js`, `core/changeEvents.js`. Cache-only paint, separate drafts, body paging, subscription repair, read ordering and greeting/capability gates. |
| E10 | `bridge/src/changes.rs` (`Kind::Tasks`, task invalidations/bounds) and `spa/src/core/cacheSync.js` task appliers. Task push is an invalidation, not a durable review/event replay service. |
| E11 | `AGENTS.md` design rules 5–6 and `ARCHITECTURE.md` RPC/push and persistence sections; #86/c/tc-01M37YEQS990896PCGFJV7KYGA. Client-safe policy/paint in SPA; autonomous work in isolated bridge services. Existing request handlers still contain policy and need extraction rather than being assumed thin. |
| E12 | `.github/workflows/ci.yml:14` (main push/manual dispatch) and tier filtering/roll jobs. Build deployment is not an automatic local review-store consumer; exact-release coverage integration is proposed work. |

Historical specs read for intent: `planning/v2/Tasks Spec.md` (formerly Issues),
`Bridge Wire Protocol Spec.md`, `E2EE Platform Scope.md`, `UI Design Brief for
E2EE Platform.md`. Their retired scheduler, old wire versions or removed methods
are not current implementation authority. This refresh changes only this plan.
