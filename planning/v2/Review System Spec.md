# Local reviews

Implementation plan for #89. Agreed by Astra / Opus 5.5 after two workshop
rounds, 2026-09-22; product questions remain for Zech in section 8.
This document proposes implementation; it does not authorize a deployment or
change product code. Source baseline: `ebeac97be65cd5adaf4f906e1b6e519bde39daf3`.

Build needs a durable place to propose a local branch, review its committed
changes, request fixes, and merge the exact revision that was reviewed. Call the
project tab **Reviews** and an individual proposal a **pull request (PR)**.
Everything runs on the owning device. GitHub, a network remote, and a running
browser are not prerequisites for retaining a PR or completing an accepted Git
operation. An agent's “Complete” message is not a review decision.

Zech's requirement, relayed during round 2 on 2026-09-22 at 21:29Z: **“any
work getting deployed needs a review by me or a Fable/Astra class agent.”**
Build's merge/deploy workflow therefore requires a qualifying approval, with
reviewer models configured per project and implementer self-approval excluded.
This is a requirement of this plan, not an open product question.

## 1. Model

### Identity and storage

One PR proposes one branch of one repository into another local branch of that
same repository. A workspace may contain several Git directories and non-Git
copies; it is not a repository identity. Several PRs can share a workspace and
issue, with independent merges. Atomic multi-repository merging is out of scope.

| Record | Required fields and meaning |
| --- | --- |
| PR | `id` (`pr-` plus ULID), per-project PR `number` displayed as `PR #N`, title, Markdown body, author (`user` or authenticated `agent_id`), timestamps, `version`, lifecycle, current revision ID, repository ID, source checkout identity, full source and target refs, workspace/directory provenance, linked issue IDs and explicitly selected trackers. |
| Repository binding | Persistent `repo_id`, canonical project path plus canonical source path/Git common directory, source provenance for independent rift clones, explicit authoritative target repository and ref. Persist the binding rather than boot-local source/project IDs. Paths are device-side locators resolved from registered scope, never arbitrary caller-supplied authority. |
| Revision | Immutable ID and sequence, PR ID, source commit `H`, observed target commit `T`, merge base `B`, source/target refs at capture, creation actor/time, retained object refs. The commits view is `B..H`; the files view compares trees `B` and `H`. |
| Review submission | ID, reviewer actor, revision ID and `H`, decision (`comment`, `approve`, `request_changes`, `withdraw`), body, timestamp, request ID if answering a request; bridge-recorded session/model facts for agent reviews. Append-only; withdrawal/supersession retains history. |
| Project review configuration | Versioned exact model-ID allowlist, writable only through authenticated user settings. SPA initial values: `claude-fable-5-1`, `gpt-6-astra`; no model IDs hard-coded in Rust. Missing configuration cannot silently disable review. |
| Implementer identities | Authenticated agent callers of create/publish plus explicitly named contributing agents; accumulated across revisions and snapshotted with each revision. A revision publisher cannot erase earlier implementers. Same-workspace membership and Git author strings do not establish implementation. |
| Review request | ID, PR/revision, explicit reviewer actor, requester, delivery operation ID, status/receipt. This is not an issue assignment. |
| Thread | ID, PR/revision, optional immutable line anchor, creator/time, open/resolved state, resolver/time and thread version. General discussion has no anchor. |
| Comment | ID, thread ID, actor, Markdown body, timestamp, optional reply-to ID. A comment does not itself request changes. |
| Merge candidate | ID, revision ID, exact `H`, current `T`, resulting tree and commit `C`, strategy, prepare ID and status. `C` has parents `[T, H]`; preparing it needs no checkout. |
| Check run | Caller-minted execution ID, repository + commit SHA + check name index, optional PR/candidate context, argv, cwd, authenticated producer, start/end, raw exit code or signal, outcome and log tail (16 KiB cap). Append runs; do not overwrite by name. Producer identity is authoritative; execution results are reported by that producer, not observed by the bridge. |
| Merge operation | Caller-stable ID, immutable input hash, actor, PR version and evidence IDs accepted, `H/T/C`, phase, timestamps, result/error, recovery facts. |
| Integration/roll evidence | Merge SHA plus explicit actor-authored completion or deployment result; separate from PR lifecycle and check results. No automatic deployment is implied by merging. |
| Retrospective range review | User-only immutable approval ID, repository, exact `from/to/to_tree`, timestamp and optional withdrawal record. Covers that explicit already-integrated range, never arbitrary future descendants. |

Do not persist `proj-N` as durable identity: current tracker storage uses the
canonical project path and maps to the live project ID on reads
(`bridge/src/app/tracker/views.rs`). Source IDs alone are insufficient if a
source is removed/recreated or its path points at a different repository.
Persist a repository binding, validate its identity on use, and mark a missing
or replaced repository unavailable. Never silently rebind history by branch
name. Project relocation needs an explicit rebind operation later.

Use the existing SQLite store at `<bridge state dir>/build.db` (default
`~/.build/tasks/build.db`), not a manifest in the workspace. Add whole-record
tables for PRs, revisions, submissions, threads/comments, requests, checks,
candidates, range reviews and merge/prepare journals; an indexed PR/issue join;
and links to the existing thread-operation receipts for explicit delivery.
Hoist project/repository, number, lifecycle, revision, timestamps and
join keys used by queries. Mint numbers and write each mutation, its timeline,
version and requested thread delivery in one transaction. Page growing timelines and lists
from day one. Schema migration is additive, with restart and backup tests.

Retain Git objects in the canonical project source repository with immutable
refs `refs/build/reviews/<pr_id>/<revision_id>/{head,target,base}` and one ref per
candidate. Import locally with connectivity verification; never depend on
alternates into a disposable workspace. This preserves old diffs through source
force-push, branch deletion, rift deletion and Git GC. Base is reachable from
head, but an explicit pin makes the recorded tuple easy to inspect; target also
gets a pin because its branch can later be reset. Metadata and refs form a
journaled two-store operation: install/verify pins before publishing a revision;
recover incomplete imports at boot. Retain all published history initially.

No additional bare archive or repository service is needed. The PR and its
diffs survive workspace removal; deleting the canonical source repository makes
Git history unavailable, and the UI says so while retaining DB metadata. Source
removal/deletion must expose this consequence rather than garbage-collect review
refs silently. A backup includes `build.db` and the source repositories including
private refs, following the store's existing consistent-backup discipline.

### States and exact revisions

Persist lifecycle `draft | open | merged | closed`. Display the requested six
states through a SPA selector: lifecycle wins for draft/merged/closed; an open
PR displays **Approved** when it has a qualifying approval, otherwise **Changes
requested** if a latest current-revision submission requests changes, otherwise
**Open**. Always show each reviewer's latest decision and any outstanding change
requests beside this summary. Keep merge progress, conflicts, checks and deployment as separate
facts. The wire returns lifecycle and submissions, not a bridge-computed
`can_merge` verdict.

At merge admission, evaluate the current project review configuration and each
reviewer's latest decision on this exact revision. Require at least one
`approve` from the paired user, or from an agent outside the PR's implementer
set whose recorded model qualifies. MCP callers cannot claim to be the user.
The user can review their own work; an implementing agent cannot approve its
own PR even if its model is allowlisted. An approval followed by that reviewer's
withdrawal or change request does not count. Another reviewer's change request
does not impose an additional veto Zech did not ask for; show it prominently.
Resolving a thread is independent. Drafts must explicitly open before merging;
closing keeps code/history; merged is terminal.

Use existing session facts, not a new attestation system: retain agent ID,
session generation, effective model chosen at spawn, observed model when
available, and fallback/uncertainty state with the submission. Full exact model
IDs only, no family-name/alias matching or agent-supplied `model` field. The
spawn model must be allowlisted; an observed model must agree with it. An
unobserved model after fallback, mismatching observation, missing identity or
unknown fallback state does not qualify. Display the reason and request a
user/qualified review. Capture facts per submission so later agent settings do
not rewrite review history. Configuration edits require a current version and
the authenticated user route; agents get reads, never allowlist writes.

Admission snapshots its qualifying submission, implementer set, model facts
and configuration version in the merge journal. Before Git effects, recheck PR
and config versions. This is a small correctness check on stored facts, not a
general rule engine. It prevents stale/accidental bypass through Build RPC/MCP.
Same-user shell access can bypass Git/DB/tool paths: it is not a sandbox or an
adversarial security boundary. Deployment coverage below detects ordinary
out-of-band integration; it cannot defend against intentional DB tampering.

Authors explicitly publish a new revision after committing fixes. Live fact
refreshes expose `source_changed`/`target_changed`; they do not silently publish
unreviewed work. A new `H` invalidates approval for the current revision, even if
the patch looks identical after rebase. Old decisions remain in history. Target
advancement alone creates a new candidate and requires fresh candidate checks,
but does not invalidate an approval of unchanged `H`. Reviewers approve the
frozen `B..H` change; checks cover its integration with current target. Keep the
original revision's `B/T` for its historical diff; candidate `T` is separately
recorded. Deliberate retargeting/base reinterpretation creates a revision and
invalidates review. Title/body edits do not invalidate code review.
Repository/source identity cannot be edited in place; open another PR.

Allow one active PR per repository/source-checkout identity/source ref/target
ref; enforce that uniqueness in storage. Branch names alone are not unique
across separate clones. An empty or already-contained source is reported as
such, never fabricated into a new successful merge. Dirty files can coexist with
a published PR but are explicitly excluded from its snapshot and diff.

### Line anchors

An anchor stores `{revision_id, old_path, new_path, side, blob_oid,
start_line, end_line, context_digest}`. Lines are positive, inclusive and refer
to the identified blob, not the rendered patch row. The bridge validates path,
blob membership, side and line range against retained objects. Additions use
the new blob; deletions the old one. Renames retain both names. Binary files,
submodules and capped text use file-level/general comments, with an explicit
reason line comments are unavailable. Read a symlink's Git blob; do not follow
it into the filesystem.

Phase 1 carries a thread forward only when the same path, side and blob OID
are unchanged. The original anchor is immutable; its current placement is a
SPA derivation. Other old threads display **Outdated — revision N**, remain
replyable/resolvable, and open their original diff. A new thread can reference
an old one. Never attach an old line number to a new blob. Line relocation may
follow app-only from immutable old/new blobs, retaining the original anchor and
leaving ambiguous results outdated. Unsent drafts
are saved separately by PR/revision; cache eviction and a revision change must
not erase typed comments.

## 2. Wire

Add `reviews.*` in a new typed `bridge/src/api/v1/reviews.rs`, with app handlers
in `bridge/src/app/reviews/` and records/store modules. These names and fields
are proposed contracts, not existing endpoints.

| Method | Inputs beyond common IDs | Result / primitive |
| --- | --- | --- |
| `reviews.list` | project, optional repository/workspace/issue/lifecycle filters, limit/cursor | Bounded versioned PR summaries, page membership, next cursor. Stable number-descending keyset pages; status styling is client policy. |
| `reviews.get` | PR, timeline cursor/limit | Canonical PR, current revision, bounded timeline and explicit coverage; aggregate record version. |
| `reviews.create` | workspace/directory, full source/target refs, title/body, draft flag, issue IDs, expected source/target SHAs | Validate scope, locally import/pin first revision, create record and links; no reviewer selection. |
| `reviews.update` | title/body, draft/open intent, linked issue set; expected PR version | Apply explicit edits with history. Source/repository are immutable. |
| `reviews.publish_revision` | expected PR version and exact source/target SHAs | Capture and pin new `H/T/B`; answer revision or existing equivalent snapshot. Never auto-commit. |
| `reviews.comment` | existing thread/reply or new thread+anchor; body; expected thread version on reply | Persist comment and optional new thread. General comments supported. |
| `reviews.resolve_thread` | thread, resolved boolean, expected thread version | Explicit resolve/reopen event. |
| `reviews.submit` | revision, decision, body, optional request ID, expected PR version | Append authenticated reviewer decision. Stale submissions may be stored only when explicitly marked historical; cannot affect current state. |
| `reviews.request` | revision, selected reviewer, explicit message/prompt inputs | Persist request and durable delivery intent atomically; return delivery receipt. Selection belongs to caller. |
| `reviews.commits`, `reviews.files` | revision, cursor/limit | Immutable paged commit/file metadata, full object IDs, counts and coverage. |
| `reviews.diff` | revision, bounded paths, optional conditional content key | Per-file patch against retained `B/H`; truncation is explicit. |
| `reviews.file` | revision, old/new side, path, bounded range/blob key | Content from retained Git objects; no arbitrary filesystem path/ref. |
| `reviews.prepare_merge` | revision, expected PR version, expected `H/T`, strategy, prepare ID | Conflict facts or retained candidate `C` and private ref for local checkout/fetch by the gate runner. |
| `reviews.record_check` | repository/commit, check name, execution ID/facts, bounded output | Store attributed evidence; cannot claim device-observed provenance as a caller. |
| `reviews.merge` | candidate, expected PR version, expected `H/T/C`, selected evidence IDs | Durable merge operation, preserving caller's explicit decision and evidence snapshot. |
| `reviews.operation` | operation ID | Admission/running/completed/failed/uncertain result and recovery facts. |
| `reviews.close`, `reviews.reopen` | expected version, optional reason | Lifecycle event; retains branches, workspaces, history and objects. |
| `reviews.settings.get`, `reviews.settings.set` | project; set takes exact model IDs and expected config version | Durable project review configuration; set is authenticated-user wire-only, never MCP. |
| `reviews.coverage` | repository, trusted deployed baseline `from`, proposed deploy SHA `to`, bounded continuation | Complete first-parent integration coverage and qualifying approval facts, or exact uncovered commits/reasons. No deployment side effect. |
| `reviews.approve_range`, `reviews.withdraw_range` | repository, exact `from/to/to_tree`, caller-minted review ID; withdrawal names existing ID | User-only wire mutations, explicit retrospective review/withdrawal. No MCP write equivalent. |
| `reviews.checks` | scoped repository/commit, cursor/limit | Append-only historical check executions, independent of the current PR projection. |
| `workspace.cleanup_integrated` | workspace, per-directory target bindings and expected current heads, cleanup ID | New explicit safe local-integration cleanup; never closes issues. Refuses if any directory's current work is not safely retained. |

Use caller-minted IDs for creates and append records (comments, submissions,
check executions and review requests), with deduplication bound to actor,
project and input content: the same ID with different content is a conflict,
never a silently ignored edit. Metadata writes require expected entity versions;
after a lost response read back the version/content before retry. Prepare,
merge and cleanup have stable operation IDs and small domain-specific durable
journals/results, queryable through their owning records/operation lookup.
Do not build a generic workflow/operation framework. Potentially expensive Git
work uses the existing deferred off-mutex execution; no promise that it always
finishes synchronously. The transport's existing `accepted: true` frame alone
is not completion. Errors distinguish stale PR,
source/target moved, conflict, unavailable source, checkout busy/dirty, missing
object, invalid anchor, and uncertain operation.

`reviews.get` returns a transactionally consistent **current** block separate
from paged timeline history: revision/H/T/B, latest decision and model facts per
reviewer, implementer set, current candidate/journal phase, and latest check
execution per name/producer for current H/C. Cap lists at 50, with explicit
`truncated`; clients show “cannot evaluate completely” rather than infer a
positive admission from a partial set. `reviews.list` carries the same compact
decision/revision block without logs/check history so list states need no N+1
read. The bridge still evaluates admission from authoritative rows, never a
caller-supplied block. Keep config as its own project/version cache record and
change item; SPA selectors combine raw decision/model facts with current config,
not a persisted `qualifies` bit that becomes stale on settings changes.

Index `current_head` and `current_candidate`: recording a SHA-keyed check bumps
only PRs currently referring to that SHA, in the same transaction. Historical
checks use `reviews.checks` and on-open refresh, avoiding fan-out across every
old revision. PR/issue/link/metadata text follows existing bounded tracker
limits (200-character title, 32 KiB body/comment); reads carry explicit limits
and coverage. Range-review UI uses `reviews.diff`/`reviews.file` with a typed
registered-repository + immutable from/to scope, alongside PR/revision scope;
the bridge validates ancestry/blob membership identically in either mode.

The bridge owns actor/scope validation, transaction safety, Git preconditions
and the narrow qualifying-approval admission check Zech required. The SPA owns
the allowlist defaults/editor, reviewer selection, display and gate requirements;
the project agent owns roll orchestration. Store explicit configuration in the
bridge and validate it at mutation time; shipping a new reviewer model must not
need a binary update. Merge has no hard-coded model names or passing-check
policy. Required check names, issue transitions and deployment remain caller
decisions. Shared fixtures test SPA and bridge agreement on qualifying evidence.

Add a project-scoped `reviews` change kind to existing `changes` subscriptions.
Items carry PR IDs and aggregate versions as invalidations, capped at 200 IDs
with explicit truncation. Every child write (thread/decision/check/candidate)
also bumps the owning PR version so a slow get cannot replace fresher details.
Emit after the store transaction commits. Register and acknowledge the
subscription **before** fetching list/detail pages; buffer invalidations while
reads are in flight, write snapshots by version, then refetch invalidated IDs
and list membership. New PRs arrive as ID invalidations too. Preserve this
ordering on reconnect, and perform a bounded fresh list/detail sync; there is
no durable replay guarantee. Overflow/truncation restarts the affected list
sync, without clearing held rows. PRs are closed, not deleted; absence from a
filtered page is not a tombstone. Track per-query local generations so a list
response begun before an invalidation cannot make its membership appear fresh.
No project sequence or general event journal is needed for this narrow design.

Add one fixture per method under `fixtures/api/v1/`, update `events.json`,
`session.hello` capabilities, method inventory and both contract test suites.
Baseline API is 1.12.0; use **the next free minor at implementation time**,
updating `API_VERSION`, `fixtures/api/versions.json` and every new fixture's
`since` together. Gate the complete feature on `reviews.local_v1`, including
operation/precondition semantics. New SPA + old bridge keeps existing Changes
and displays an update requirement for Reviews. Old SPA + new bridge retains
its existing contract. Do not send unknown mandatory safety fields to legacy
finish/merge methods: the current facade silently drops them.

## 3. Git mechanics

### Local publication and target authority

The workspace branch is the source. A PR publishes references to commits, not
a copied patch and not uncommitted/index state. Worktree sources share objects
with their parent repository; rift sources have independent refs/object stores.
For both, capture exact `H` from the registered source checkout, import/pin it
in the canonical project source repository, and verify the imported commit.
For rifts use a local fetch like `git fetch --no-tags <registered clone>
refs/heads/<branch>:refs/build/reviews/<pr>/<revision>/head`, with a fresh private
ref and no force prefix, then verify its OID equals `expected_head`. Reject a
race and do not publish the revision on mismatch. Do not call the current
`publish` helper: it force-updates shared `refs/heads/<branch>` and another
clone may use that name. An external push is unrelated to updating the PR.
Reads always use the private retained refs, never a workspace's mutable HEAD.
Publication is explicit; phase 1 does not depend on clone/common-ref filesystem
watchers. Refresh/reopen/prepare re-read live facts before an action.

The target is an explicitly registered local repository/ref, defaulting to the
source's project repository and configured base branch (usually `main`). Never
infer `origin/main` or a same-named branch in an arbitrary integration clone.
An existing integration clone can be selected as authority only through an
explicit binding; the UI names where the merge will land. Additional source
directories need this mapping before they can open PRs. Detached HEAD,
non-Git directories, unrelated histories, source equal to target and unresolved
repository identity are precise refusals.
When authority is a separate registered local clone, capture/import its exact
target `T` into private refs before preparing in the canonical source repo, then
import and verify exact candidate `C` in the authority before landing. Query
target ancestry/coverage there. Object transfer is local in both directions and
never changes a shared source branch or relies on a network remote.

### Candidate, checks and merge

Recommend a **no-fast-forward merge commit**. It preserves reviewed commits and
anchors and gives each PR a visible integration boundary; it also matches
Build's present integration workflow. Squash would require mapping the reviewed
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
   tree, author/message and SHA, and pin it. Include PR/revision trailers in the
   message. Reuse the persisted candidate for the same parents and merge inputs,
   including metadata; do not recreate it with a new timestamp on every read.
   Require Git 2.38+ and detect the needed flags on the owning device; refuse
   prepare with an upgrade explanation if absent. The checkout-free merge mode
   arrived in [Git 2.38](https://raw.githubusercontent.com/git/git/v2.38.0/Documentation/RelNotes/2.38.0.txt).
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
   It supplies evidence IDs, expected PR/config versions and exact candidate
   tuple. New review decisions or revision changes invalidate an old admission.
   Target advancement needs a new candidate and checks; approval remains bound
   to unchanged source revision `H`. Checks of `H` alone do not cover `C`.
4. **Apply.** Persist a write-ahead operation containing `H/T/C` before target
   mutation. Revalidate qualifying approval, lifecycle/versions, object
   connectivity, target identity, target SHA and candidate tree. Advance the named target from `T` to
   exact `C`, never recompute a different merge after checks. Serialize competing
   Build operations; preserve the caller's chosen merge/evidence in the receipt.
5. **Settle.** Confirm target ancestry and checkout state, mark PR merged and
   append the merge event transactionally, then publish its change event.
   Network pushing is a separate caller step and, for Build's main branch, a
   deployment action gated by coverage. Issue completion, deployment and
   cleanup are separate explicit steps and may fail separately.

**Land in the actual target checkout.** Enumerate `git worktree list --porcelain`
in the authoritative repository. If target is not checked out anywhere, use
`git update-ref <ref> C T`, a true ref compare-and-swap. If checked out at path
`K`, require HEAD on the target at `T`, no tracked dirt/in-progress Git state,
and no active Build writer whose cwd is that checkout. This is checkout-scoped,
not project-scoped: the project agent runs from its own scratch directory and
must be able to call merge. Then run `git -C K merge --ff-only C`, allowing Git's
untracked-overwrite checks to refuse. Never stash/reset a user's work. If the
target is checked out more than once, refuse the ambiguous arrangement.

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
| Git success, issue update/roll/cleanup fails | PR stays merged; only the separate failed effect is retried. |

No cross-store atomicity is claimed between SQLite and Git. The retained `C`
and journal make reconciliation possible. Test termination at every boundary,
including after ref change but before database commit and after commit before
response. A timeout is a reason to query `reviews.operation`, not merge again.

Keep the workspace, source branch and PR after merge. A subsequent source
commit does not reopen a merged PR; create a new one. Explicit cleanup
checks every workspace directory, active agents/terminals, dirty/untracked
files and reachability of all local work. It must not delete an adopted/user
checkout or another PR's only copy. Retained review refs preserve historical
review objects but are not blanket permission to delete unrelated workspace
work. Do not route cleanup through `workspace_finish_legacy`: it drops the
branch delete action (#87), and `workspace.finish` currently couples remote
publication with closing all linked issues. First-release PR merging does not
call either flow.

Ship a separate `workspace.cleanup_integrated` path in phase 1 so a project
with no remote can finish. For each Git directory, freshly measure its **current
checkout HEAD**, import that exact object into the authoritative repository
when needed, and verify it is reachable from the named current local target.
Do not test the last reviewed `H` or the project's same-named source branch:
rift publication intentionally left that branch untouched, and subsequent
unpublished commits must block deletion. Refuse dirty/untracked/plain-directory
work, active writers or missing objects; preserve all existing adopted/path
ownership guards. Recheck heads/status before removal under the existing
retirement/deferred-cleanup discipline and journal partial removal. Private PR
pins alone do not prove unrelated branch work has been integrated. Do not
change legacy `workspace.finish` semantics or auto-close linked issues.

### Deployment coverage

Build's current CI deploys on pushes to main and manual workflow dispatch
(`.github/workflows/ci.yml`). The review system remains entirely local, but
Build's own rollout procedure must check review coverage **before a main push,
CI dispatch, or local bridge/app restart that deploys new work**, not afterwards.
`reviews.coverage` walks first-parent commits between an explicitly trusted last
deployed SHA and the exact proposed release SHA. Require the baseline to be an
ancestor; page with explicit completeness, never treat the first page as all.
Each integration commit must match a retained completed PR candidate with its
qualifying approval evidence. Direct commits, `H`-only external integrations,
missing receipts and unqualified approvals are uncovered and block the normal
roll procedure. The candidate binds all source-side work in that PR; earlier
target integrations are covered independently along first-parent history.

At deployment re-evaluate retained submissions against current project config
and latest decisions, in addition to the evidence snapshot at merge. A removed
reviewer model or withdrawn approval can therefore block a still-pending roll.
`reviews.submit` remains available on merged PRs: an approver's later withdrawal
or request-changes means that approval no longer qualifies for deployment.
Another reviewer's change request does not veto qualifying evidence. A new
qualifying approval of that unchanged revision or a user retrospective range
approval restores coverage; history and the completed merge stay unchanged.
Return commit/PR/evidence/config versions and reasons, never accept a caller's
`reviewed: true`. The project agent verifies the exact to-SHA and complete
coverage immediately before the deployment action and records the result; a
new main commit requires another check. Show uncovered commits in the SPA.

This does not make CI consult a local database or prevent a same-user shell
from pushing anyway. Implement the check in the project-agent roll procedure
and any Build-owned deployment entry point; manual shell bypass remains a
workflow violation. Strong prevention would require a separately trusted deploy
gate/CI evidence transport, outside this local PR scope. PR trailers identify
work but are not approval evidence. Initial adoption needs a user-selected,
verified deployed baseline; it does not retroactively certify older history.
Include retrospective user review in phase 1, subject to Zech's product answer
below: show the exact from/to diff, then record `reviews.approve_range` with the
Git-verified to-tree. Coverage accepts only that repository and explicit
first-parent range; descendants outside it still need evidence. The baseline
must lie on the first-parent chain, not merely be a reachable side-parent.
Range endpoints, tree, actor and approval ID are immutable; user-only withdrawal
removes that evidence. No agent can manufacture this user approval through MCP.
This makes direct user commits/external merges reviewable before deployment
instead of wedging the rollout. There is no silent retrospective exemption.

## 4. SPA

Add `#/project/<projectId>/reviews` and
`#/project/<projectId>/reviews/<prId>` through `core/router.js` and the
`app.js` view table; preserve device-qualified routing. Register a project
standing for the detail route in `core/shell.js` and a Reviews tab in
`core/toolbarModel.js`. Issue and PR navigation keep the shell and project rail
mounted. The PR links to author/reviewer conversations; it does not invent a
second conversation owner or replace the issue's assignee.

The list has title/number, branches/repository, author, derived review state and
check/merge summary, with bounded Open/Closed/All queries. The detail page has
description and linked issues, activity/decisions, commits, files/diffs,
thread replies and resolve, check runs, and explicit open/draft, request review,
approve, request changes, prepare/check, merge, close/reopen actions. On mobile
these are sections of the same page; the persistent shell stays visible. The
header says which revision is displayed and when live branches have moved.
The page shows reviewer model identity and why an approval does or does not
qualify, and disables Merge without a qualifying approval. Current user-owned
review configuration is cached like other entities; setting changes invalidate
approval/coverage selectors. Missing/failed gate evidence and outstanding
change requests remain visible beside the decision. No arbitrary “merge anyway”
option bypasses Zech's qualifying-review requirement.

Workspace Changes remains the editing view, including uncommitted work. It gains
**Open pull request** or **View pull request**, scoped to the selected directory.
The PR page reviews committed objects only. Existing `taskReview.js` and
`worktreeReview.js` remain for legacy browsing; they must not pretend a
conversation request or local file checkmark is PR approval. When a current
branch has an active PR, the new SPA's direct merge entry links to it. Old
clients/direct Git can still merge; detect external integration and present it
truthfully instead of claiming PR approval was enforced globally.

Reuse the diff stack, lazy per-file bodies, viewport, folding, place keeping,
escaping and composer freeze behavior from `changesReview.js`, `diffRender.js`,
`changesetBodies.js` and `changesComments.js`. Extract a small comment adapter
boundary for durable review threads and overlay existing comments; retain the
legacy `run.request_changes` adapter. PR revision IDs are Git review revision
IDs, not conversation `currentRevisionId`. File “viewed” marks remain personal
UI state. Do not extend the already large plug into a PR controller.

Add `reviewCache.js`, `reviewSync.js`, `reviewModel.js` and a mutation/controller
module. Cache canonical PR records separately from list membership and timeline
pages, keyed by device + project + PR/revision. Never evict review records with
workspace cache data. All record reads and events take this route:

`RPC/event → version-aware reducer → cache commit → notification → cache read → paint`

Mount from cached data immediately; absent data and a cached empty list differ.
Refuse older record versions, including a slow pull landing after a push.
Invalidate queries on membership-changing events and preserve data while
repairing gaps. Optimistic comments live in cache with operation IDs; retries
reconcile rather than duplicate them. Draft text has separate persistence.
If persistent cache is unavailable, use the common volatile cache backend or
an explicit cache error, never a payload-only entity paint path. Follow #82's
cache infrastructure as it lands instead of copying a competing store.

Cache commits/diffs/blobs by repository/revision and old/new blob/path/options
through the existing lazy-body seam. The issue permits an uncached Git-read
exception, but this feature chooses caching for repeatable historical review.
They must never be keyed only by mutable branch name. Truncated
diffs say so and support bounded continuation; a missing body is not an empty
file. Offline cached reviews remain readable, but no optimistic approval/merge
claim is final until admitted by the bridge.

## 5. Agents

Expose project-scoped MCP tools mirroring the primitives: `list_reviews`,
`get_review`, `open_review`, `update_review`, `publish_review_revision`,
`comment_review`, `resolve_review_thread`, `submit_review`, `request_review`,
`read_review_diff`, `read_review_file`, `list_review_commits`,
`prepare_review_merge`, `record_review_check`, `merge_review`,
`get_review_operation`, `close_review`, and `reopen_review`. Group large read
options rather than require one tool per file. Tool names are provisional until
the typed contract review; signatures and fixtures must agree.
Also expose `check_deploy_coverage` and read-only review configuration. Prepare
and merge tools are on the Project MCP surface only; authors/reviewers use the
Coding surface for proposals and reviews, and the paired SPA can merge. Enforce
this in `BridgeAction::surfaces` and dispatch, not merely discovery. No MCP
tool can write the reviewer-class allowlist or create a user/range approval. New
settings/coverage/cleanup wire methods each need fixtures and capability gates.

Like tracker tools, project and author come from the authenticated conversation
binding, not arguments. Validate scope in app handlers and `BridgeAction`
surface authorization, not only tool discovery. An agent cannot read another
project by guessing a PR/blob/operation ID. Caller-stable operation IDs survive
compaction; mutation tools return receipts and current versions for the next
step. Merge is an explicit supported capability; hiding a tool is not an
authorization check.

The author commits, opens a PR linked to the held issue, and requests review.
The project agent or user chooses a qualifying reviewer using the configured
exact-model allowlist and existing role preferences,
creates one with existing workspace/agent tools if needed, then sends an
explicit review request. Write the request and its thread post/operation receipt
in one `build.db` transaction, reusing the existing queued/claimed/delivered/
uncertain receipt path and request-recipient identity for retry. The bridge
does not choose the reviewer or launch a new review every time a file changes.
Existing delivery executes accepted requests with the browser closed. Phase 1
does not promise an autonomous general workflow engine; the project agent
continues the explicitly requested orchestration and can resume from PR facts.
Give PRs explicit trackers selected by the controller (normally author,
reviewer and project agent). Explicit review requests and submission deliveries
reuse durable thread-operation receipts, committed with their review records;
do not add a general notice outbox. Other comments/state notices reuse the
tracker's best-effort scoped notice/delivery and settle-window machinery;
exclude the actor and expose delivery failure for explicit requests/submissions.
A burst coalesces into one turn that reads `get_review`. Do not rely on Complete
reports being automatically forwarded. The current tracker notice path is not
already durable transactional fan-out; the explicit review write/post transaction
is a concrete store integration task in phase 1.

Reviewer dispatch carries PR ID, exact revision/H/T/B, linked issue IDs, review
scope, expected result and a `get_review` instruction. A reviewer reads immutable
diffs/blobs or a checkout verified at `H`, not the author's moving working tree.
It writes durable findings and one decision; a late result stays attached to
the revision it actually read. Author fixes go through commit + publish revision
+ request re-review, locally; “push fixes” does not mean a network push.

The project agent reads the PR instead of a “ready to roll” message, evaluates
the qualifying-review configuration and chosen gates, prepares/checks/merges
exact `C`, checks complete deployment coverage and runs the separately
authorized roll. It records deployment/verification evidence
and then explicitly advances the appropriate issues. It resumes uncertain work
by operation lookup, not by repeating a merge. Gates can run without a browser;
closing an agent session interrupts that agent's unaccepted future steps, not
an already durable Git operation.

Update shared active instructions in `bridge/src/templates.rs`, corresponding
MCP descriptions in `bridge/src/mcp.rs`, and
`bridge/src/orchestrator/workspace.rs` conversation/delivery envelopes together:

- Open/publish a PR for committed branch work; read the linked issue and current
  PR/revision before acting. Never put uncommitted edits in the review claim.
- Post on state change, a decision/question/blocker or requested evidence.
  Do not acknowledge delivery, report “starting,” or echo another agent's recap.
- A review submission, an implementation Complete, a merge and a verified roll
  are distinct facts. Complete cannot approve a PR or imply a successful gate.
- Review findings live on the PR; issue comments report decisions and blockers
  relevant to scope. Do not copy the entire diff or every review reply there.
- A review request must not reassign the implementation issue to the reviewer.
- Before a deployment/main push, require complete reviewed coverage through
  the exact release SHA. Only the user or an allowlisted independent model's
  approval qualifies; shell access is not permission to bypass that rule.

Keep this initial prompt change capability-gated. #86's future app-owned prompt
policy can replace the text later; do not make this PR feature depend on a new
headless policy runtime or port retired plan/stage scheduling.

## 6. Issues integration

Store the many-to-many link once, with roles `relates_to` or `completes` chosen
explicitly by the controller. `reviews.list {issue_id}` and an additive issue
detail projection expose PR summaries; issue rows show review links/state from
cached review records. Links survive workspace cleanup and do not need an
unqualified branch name to recover repository identity. Review and issue link
events are written atomically when created in the same store.

Merge can move selected completing issues to Done when **the caller explicitly
requests it** and there is no remaining required delivery step. Model this as a
separate idempotent, expected-version issue update linked to merge SHA; a failed
issue update cannot roll back Git. For Build the recommended policy waits for a
verified roll, records its result, then moves Done. For a project without a
deployment step, the controller can send the Done update immediately after the
successful merge receipt. No automatic `closed` state is implied; issue closure
and the board's Done column remain separate.

If several PRs complete an issue, the controller records the intended set and
finishes only after all are merged and required delivery is verified. Merely
linking an issue is not a promise to complete it. A closed-unmerged PR does not
complete any issue. Additional unrelated linked issues are untouched.

Current `app/tracker/activity.rs` moves a held issue to In review on Complete
and closes all linked issues on workspace finish. These existing hooks conflict
with this workflow and must be addressed in phase 1, not just contradicted by
prompts. Add an explicit, durable review-workflow intent to the issue's dispatch
context for new PR work: completion reports do not apply the legacy automatic
In-review move to that dispatch; the controller records its intended transition.
Preserve the old dispatch behavior for old clients/non-PR work until separately
migrated. PR cleanup must never call the close-all-issues hook. This is a
capability/versioned behavior addition, not an unknown field on legacy finish.

In review means **Zech needs to look**. Agent-only review can leave the issue
In progress while the PR records review progress. This planning issue itself
belongs In review when complete because it explicitly requests Zech's decision.
Keep exact issue/turn/assignee fencing from #48; do not move every tracked issue
when any agent finishes. Issue auto-transitions and completion policy remain
explicit controller decisions; the bridge only applies accepted intents.

## 7. Rollout

The first public release is one usable vertical slice, activated only when all
of phase 1 passes. Internal commits may land behind `reviews.local_v1`; an
unfinished record-only API is not announced as a working review workflow.
Coordinate the required bridge roll because it interrupts agent sessions. No
roll is part of this planning issue.

Sizes are engineering/review estimates, not promises: S = 1–2 focused engineer
days, M = 3–5, L = 6–10. Parallel implementation can shorten elapsed time after
contracts are agreed; storage/Git correctness remains on the critical path.

| Phase | Files / deliverable | Validation / exit | Size; roll |
| --- | --- | --- | --- |
| 1a: records and wire | New `bridge/src/reviews.rs`, `store/reviews.rs`, `app/reviews/{mod,edits,reads,tools}.rs`, `api/v1/reviews.rs`; register in module roots, `app/rpc.rs`, `store/schema.rs`, `store.rs`; domain journals, user-owned review settings/range approvals; reuse `store/operations.rs` for explicit delivery; typed fixtures/version files. | Restart/migration, actor/scope fences, repository identity, number/active-PR uniqueness, expected-version conflicts, same-ID/different-body refusal, bounded current-state vs timeline reads, pin-before-publish recovery, wire-only config/range-review writes; both contract suites. | L; bridge, feature hidden. |
| 1b: Git, evidence and cleanup | New `app/reviews/git.rs` and coverage read; reuse `app/git/deferred.rs`, `worktree`/isolation helpers; candidate/journal/check storage; new integrated-cleanup handler beside `app/workspaces/deletion.rs`, explicit ancestry facts beside `gitgui/unpushed.rs`/`workspace.rs` without changing remote push counts. | Worktree/rift local-only flow; dirty/in-progress target, checkout-specific writer guard, parents/tree identity, target moves, racing merge, hooks, conflict isolation, no-ff, historical GC after workspace deletion, interrupted checkout, crash boundaries; all-directory/current-rift-head cleanup and extra-commit refusal. | L; bridge, feature hidden. |
| 1c: cache and surfaces | New `core/review{Cache,Sync,Model,Actions,Comments}.js`, review list/detail modules; `changesReview.js` adapter seam; `app.js`, router/shell/toolbar/project views, `cacheSync.js`, `changeEvents.js`, bridge API adapter; settings UI/styles. | #82 cached mount with delayed/absent payload; real cache-write-to-redraw; slow pull/new push; subscribe-before-snapshot and reconnect/truncation; paged membership; revision/draft preservation; durable threads on both sides/rename/deletion; 390px shell; immutable diff truncation/escaping; config-change redraw. | L; app plus 1a/1b bridge capability. |
| 1d: workflow and activation | `mcp.rs`, `app/mcp.rs`, review admission/request modules; existing session/model observations, templates/workspace prompt; `app/tracker/{activity,dispatch,views}.rs`, tracker store/typed contracts for explicit dispatch and issue-update intents; `trackerIssueView.js`, tracker cache/sync and Changes entry. Build's project-agent roll procedure calls coverage before push/dispatch/restart. | Full author → qualifying review → fix/re-review → candidate checks → merge → coverage → explicit issue completion. User vs MCP, configured/observed model mismatch/fallback, same-workspace independent reviewer, publisher self-review, contributor persistence, withdrawal/config race, direct/external uncovered commits, changed deploy SHA, missing baseline/incomplete coverage, browser closed, duplicate delivery, roll/issue-update failure, old/new clients. | L; coordinated bridge + app activation of all phase 1. |
| 2: navigation and policy polish | SPA review list/query/selectors, issue badges, saved filters, reviewer/check policy UI within shipped primitives, compare revision selection, personal viewed marks. | Cache/route/mobile regressions; policy selection consistency with recorded explicit merge decisions. | M; app only if no wire semantics change. |
| 3: optional device capabilities | Stronger check execution provenance; retention primitives and any persisted anchor-remapping data in review modules/fixtures. Exact-blob display carry-forward already ships in phase 1; further client-only mapping needs no bridge roll. | Interrupted checks, mapping ambiguity, retention preserves referenced objects and drafts. | M–L per device capability; bridge + app. |

Phase 1c/1d also includes the uncovered-range review UI and tests: exact
from/to/tree display, user-only approval/withdrawal, no coverage of later
descendants, invalid first-parent baseline, post-merge approval withdrawal,
replacement qualifying approval, and MCP attempts to impersonate the user.

Phase 1 totals roughly 24–40 engineer days including review and failure-path
tests; scope can be split among implementers after the wire/model contract is
fixed. Do not trim exact object retention, merge recovery, rift support,
durable comments, cache semantics or truthful check results to fit one roll.
Defer rich notification inboxes, automatic anchor relocation, auto-merge queues,
stacked PRs, squash/rebase merge options, GitHub integration, multi-device
repository replication and atomic multi-repository changes.

Implementation gates: bridge format/clippy and applicable Rust tests including
API contract fixtures; SPA lint and full Vitest; gitleaks/semgrep on changed
files; `git diff --check`, all by recorded exit code. Add real temporary-repo
Git tests, not only mocked return values. Demonstrate a repository with **no
remote configured** through create, review, fix, merge, restart and historical
read. UI acceptance must include cached offline history after workspace removal.

For this document-only issue, verification is source/path/contract review,
coverage of all eight requested sections, partner review in two rounds, and
diff checks. Running application suites would not validate a Markdown proposal.

## 8. Open questions for Zech

These are product choices with recommended defaults, not missing prerequisites
for finishing this plan. The implementation brief should record accepted
answers before activating the corresponding behavior.

| Question | Recommended answer and consequence |
| --- | --- |
| What should the UI call this? | **Reviews** project tab; **pull request** for each local branch proposal. Familiar terms without suggesting GitHub or remote publication. |
| Should PRs share issue numbers? | Keep independent sequences and explicitly display `PR #N`; shared numbering needs tracker migration/link-resolution work and is unnecessary for the first release. |
| Which merge strategy? | No-ff merge commits only initially; preserve commit identity and an auditable PR boundary. |
| Is one PR per repository acceptable? | Yes. Group PRs through issues/workspace links; show incomplete groups, and avoid promises of atomic multi-repository merging. |
| What happens after merge? | Retain workspace/branch and history; offer explicit safe local-integration cleanup, never automatic deletion or issue closure. |
| When does a linked issue become Done? | Explicit completion intent: after verified roll for Build; after merge for projects with no further delivery step. Default links are informational, not completion promises. |
| How conservative should stale approval be? | New source/retargeted revision requires re-review; ordinary target advancement retains the source review and requires fresh candidate checks. |
| Is actor-attested gate evidence sufficient initially? | Yes, matching today's project-agent execution, clearly labeled and pinned to candidate SHA. Add device-observed execution separately if stronger provenance is wanted. |
| What target checkout arrangement should phase 1 support? | The actual clean target checkout, guarded ff-only of prepared C; ref CAS if not checked out. Refuse dirty/in-progress/active-writer checkouts, never stash/reset. |
| Should Build disable Git hooks during prepare/land? | Yes, to keep the exact-candidate primitive predictable. This suppresses post-merge/reference-transaction hooks; run intended validation through explicit gates. |
| Should the user be able to approve an already-integrated uncovered range? | Yes; include an explicit user-only exact repository/from/to/tree review in phase 1, so direct user commits can be reviewed before roll. No agent equivalent or approval inferred from reachability/trailers. |
| May the reviewer also perform the merge? | Yes, if it is the user or a qualifying non-implementer agent. Reviewer and implementer must differ; requiring a third merger is not implied by the stated rule. |

## Workshop and code evidence

Round 1 draft was posted on #89 before sending it to Review partner, and both
agreed the first revision. Opus proposed checkout-free candidate construction,
real-target ff-only landing, SHA-indexed checks, exact-blob anchors, reused
notices and local-only cleanup; Astra accepted. Astra retained draft, a small
merge journal, independent PR numbering and explicit issue completion; both
agreed. Private-ref rift import replaced existing force-publication to avoid
clobbering same-named branches. A second bare archive and general workflow/replay
framework were removed to keep the first release focused.

Round 2 incorporates Zech's mandatory qualifying-review requirement, replacing
round 1's optional-review recommendation. The challenge added user-only config
writes, authenticated session/model evidence, explicit implementer identities,
shell-bypass limits and deploy-time coverage. Target advancement preserves the
source review but changes the candidate/checks. Opus reviewed the full document
and agreed after four clarifications: event publication never means network
push; gate runners can use existing controlled checkouts; retrospective user
range review ships in phase 1; post-merge submission/withdrawal semantics are
explicit. Both accepted the final resolutions, with no outstanding blockers.

Scope decisions: reuse durable thread-operation receipts instead of a new
notice outbox; retain the narrow completion-intent hook because a new workflow
must honor “In review is for Zech”; retain local-integrated cleanup because the
brief requires operation without a remote; retain small domain operation lookup
because cleanup can remove the workspace before a lost reply is recovered.
Prepare/merge MCP tools are project-only. A complete bounded current block
serves selectors independently of timeline pages; truncation is never approval.

Required code read before drafting: `spa/src/core/changesReview.js`,
`spa/src/views/worktreeReview.js`, `spa/src/views/taskReview.js`,
`bridge/src/app/workspaces/`, `bridge/src/app/tracker/`, `bridge/src/mcp.rs`,
`planning/v2/Issues Spec.md`, `planning/v2/Bridge Wire Protocol Spec.md` and
all five #86 audit comments. Additional seams inspected: `store.rs`,
`app/runs/review.rs`, `worktree/mutation.rs`, `lifecycle/publication.rs`,
`core/{router,shell,toolbarModel,trackerCache,branchFinish}.js`, templates and
workspace records, `gitgui/unpushed.rs`, `app/projects/conversation.rs`, tracker
notices, isolation publication and `.github/workflows/ci.yml`.

The main constraints confirmed in code are: review notes currently post to
conversations; workspaces contain multiple sources; tracker project IDs are
mapped from durable paths; workspace Done is destructive and closes linked
issues; legacy finish drops action; API is 1.12.0; the existing publication
journal precedes Git effects but is not a complete PR candidate transaction;
the shell is shared; typed request adapters discard unknown fields. #86's
device safety/transaction boundary is retained. Zech's qualifying-review rule
adds a narrow admission check using persisted configuration; model lists,
reviewer choice, display, gate requirements, issue completion and roll
orchestration stay out of hard-coded bridge policy.
