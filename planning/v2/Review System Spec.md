# Local reviews

## Owner summary

The task is the review. Release 1 can land reviewed changes on Build's main.
Attach one branch, review its diff, and leave line comments on the task.
Existing assignment tools pass work between implementer, reviewer and deployer.

Recommended choices:

- Push to origin main; sync updates the base checkout when it is safe.
- Let the project agent or user merge. An agent never merges its own approval.
- Choose the reviewer yourself and keep a list of allowed models.
- Allow approval when the reviewer's configured model is on your list,
  even when the agent cannot report the model it actually ran.
- Run checks on the exact merge commit. If origin moves, rebuild and check again.
- Mark Done only after deployment is verified. Keep the workspace and branch.

Estimate: **11–17 engineer days**. Assignment reliability ships first, then review,
then merge. Multiple branches, automatic handoffs and cleanup come later.

## What was stale in the September plan

| September assumption | Current evidence / revised decision |
| --- | --- |
| Separate PRs, `PR #N`, a Reviews project tab, PR comments beside issue comments | Superseded by #145: task identity and timeline, task Changes/review surface, branch attachments. Release 1 supports one; several follow under #145. No PR-to-task join or second numbering sequence. |
| A review request must not reassign the implementation issue | Superseded by #145: explicit handoff assigns the same task to its reviewer, back to its implementer for fixes, then to its deployer. Contributor identity persists independently of the current assignee. |
| Every step waits for a human, or workflow runs only in a connected SPA | #145/c/tc-01M3A6P0V88S483NXWH48CP83K makes human an explicit step type; #86/c/tc-01M37YEQS990896PCGFJV7KYGA keeps always-on work in bridge services. |
| API 1.12.0; `issues.*`; `Issues Spec.md` | API is **3.5.0**, tracker is `tasks.*`, spec is `Tasks Spec.md`. #322 advanced the wire during this challenge and corrected the architecture's earlier 3.2.0 summary. `api/v1/` paths still exist. [E1] |
| Unknown top-level request params are silently discarded | `parse_params` now refuses unknown top-level fields; nested types still need explicit validation. Named capabilities remain required. [E1] |
| `tasks.link` is an available wire primitive | Removed from the wire in 3.0; `link_task` MCP still exists. Add explicit task-review attachment primitives, not calls to removed verbs. [E2, E7] |
| `workspace_finish_legacy` drops `action: delete` (#87) | Fixed: it forwards the selected branch into `finish_workspace`. Do not plan to fix it again. [E4] |
| Every workspace finish closes all linked tasks | Too broad now: closure is conditional on a merged run. Still unsuitable for review cleanup, which must not close unrelated tasks. [E4] |
| Cleanup needs a new independent lifecycle system | Reclaim, reservations, path containment and checked branch deletion now exist. Keep them. Local integration cleanup is deferred; preserve existing behavior for non-review tasks. [E4, E5] |
| The review target has no other background writer | Source-base synchronization now fetches/fast-forwards bases. It has its own path lock. Push the reviewed commit to the selected remote, then request existing base sync; only direct local-ref landing refuses a checked-out target. [E6] |
| Reconnect reads first and subscribes later; drafts share the evictable replica | Current sync repairs late subscription coverage and fences pushes; drafts/UI state have their own `build-ui` store. Extend those seams, do not rebuild them. [E9] |
| New active review UI can extend legacy `taskReview.js` plan approval | Singular `task.*` is historical plan data; several mutations were removed. Tracker tasks use `trackerTaskView.js`. Keep legacy browsing separate. [E1, E8] |
| `templates.rs` is the only prompt source, and model configuration proves the model that reviewed | Prompt text is in `bridge/templates/`; capture configured and reported models separately. Configured-only qualification requires a visible user setting. [E7] |

The earlier exact-snapshot Git design, local private refs, task history and
independent review remain. The October challenge removes the extra first-release
workflow, coverage, cleanup and multi-repository machinery. The workshop log
records why each prior decision changed.

## 1. Model

### One task, one branch attachment, immutable revisions

Reuse the tracker's task ID, number, title/body, author, assignee, board column,
open/closed state and timeline [E2]. Existing task `attachments` are uploaded
files. Add a distinct `branch_attachments` field, constrained to **zero or one**
in release 1. Keep the plural field as an extension point, not a promise of
multi-repository support. Branch strings in `links` remain navigation only.

| Task-owned record | Release-1 contents |
| --- | --- |
| Branch attachment | Stable child ID, repository identity, registered source workspace/directory/ref, explicit target binding, author and accumulated implementer identities. Binding survives reassignment. |
| Target binding | User-selected, versioned project binding: canonical repository, mode `remote` or `local_ref`, full target ref; for remote mode, selected remote and safe endpoint identity, plus the source that follows it. Agent attach only references this binding. Build selects origin/main. |
| Revision | Sequence and immutable ID; attachment, source H, observed target T, merge base B, refs, actor/time and retained Git pins. The revision **is the review round**. No round table, revision-set approval or successor/partial-merge state. |
| Review slot | Designated recipient (user or concrete agent), kind `human` or `agent`, designating user/time, and current input revision. Only the user sets recipient/kind; successful publication advances the input revision while keeping that designation. Existing task assignment records who dispatched the reviewer separately. |
| Submission | Append-only approve/request_changes/withdraw decision on one revision, rationale task-comment ID, authenticated actor and model provenance. Current decision is the latest decisive submission by the designated recipient. |
| Thread | Immutable file/line/hunk anchor and reply/resolution metadata. Text and replies live once in `tracker_comments`, in the ordinary task timeline. |
| Candidate | Revision and exact H/T/C/tree, merge metadata and retained candidate ref. C has parents `[T, H]`. |
| Merge operation | Stable caller operation ID, expected review version, target binding and H/T/C, admitted decision/check-comment references, pending or settled result, actor and timestamps. Remote integration, base sync and deployment are separate outcomes; Git and SQLite are separate stores. |
| Project review settings | User-only target binding, exact provider/model allowlist, configured-only qualification switch, required check commands, and settings version. No model IDs or gate names hard-coded into Rust. |

Store additive child rows in the existing `build.db` (default
`~/.build/tasks/build.db`), beside tracker storage [E3]. Include them in project
deletion and backup handling. No PR table or separate comment store. Each task-review mutation commits
its child records, canonical task comment/event, operation receipt and one
aggregate `review_version` in the same SQLite transaction. Review-relevant task
assignment, contributor, lifecycle and configuration changes bump that version
too. Keep current state bounded and separate from the growing timeline.

Use canonical project/repository identity as well as paths [E2]. Resolve paths
from registered scope, never arbitrary caller input. Refuse a missing/replaced
repository rather than silently attach old evidence to another clone. Pin
H/T/B/C in the canonical source repository under
`refs/build/task-reviews/<task>/<revision>/...`; import before publishing the DB
record and reconcile an interrupted pin/write. Published objects must survive
workspace deletion and Git GC without alternates to disposable workspaces.
Retain history initially. A restorable backup needs SQLite **and** those Git
objects/refs. Missing Git bodies are unavailable, never empty diffs.

Do not replace the attachment after it has history. A new H on the same source
creates a new revision and invalidates approval, including an equivalent rebased
patch. Retargeting after publication uses a new task in release 1. After merge,
follow-on work also uses a new task. A second repository requires another task;
#145's later multi-attachment slice can migrate revisions into rounds then.

### States, designated review and provenance

Derive the requested display states in the SPA: Draft, Open, Changes requested,
Approved, Merged, Closed. They do not create tracker columns. Attach starts Draft;
publish explicitly opens the committed revision. Merged means C was integrated
into the named target (origin/main for Build), not that the base checkout synced
or deployment succeeded. Closure preserves history.
Target-only movement retains source review on H but invalidates the candidate
and its checks. An already-contained H is an external-integration fact, not a
successful merge by this feature.

Only the **current designated review recipient**, while holding the review
assignment for the current revision, can submit a decisive approval or change
request. Other actors leave ordinary findings. A designated reviewer may later
withdraw its own decision after handoff; that invalidates an unmerged admission.
Holding the review assignment means current assignee equals designated recipient
and the persisted handoff input equals the current revision. Recheck both at
submission. Later reassignment for checks/merge does not erase that approval.
Changing the recipient is user-only and invalidates earlier approval. Record
both the user who designated the reviewer and the actor who dispatched it.
`assign_task` never grants approval authority or rewrites the designation.

Agent reviewers must be outside the persistent implementer set. Publication and
explicit contributor declarations accumulate; handing the task away does not
erase authorship. A project agent publishing for an implementer records both
identities. Creating an agent or claiming a role does not qualify it. Review in
another workspace is the convention, but workspace membership and Git author
strings do not prove independence. The authenticated user may select themselves
as reviewer; an agent cannot manufacture that selection or a user approval.

The agent performing merge cannot rely on its own approval, even if it did not
implement the work. Release-1 MCP merge remains on the Project surface; the
project agent delegates review to a different designated agent. A user can both
review and merge. Designated agents need not be process-isolated from malicious
same-user shell access; this is accountable workflow, not a sandbox.

Capture `{configured, reported, source}` at submission, plus provider, agent,
authenticated session instance and time. `reported` may be absent or a provider/
PTY observation. `source` distinguishes provider report, PTY parse, configured
only and known fallback. Preserve durable session lineage; do not accept a model
string supplied in the approval payload. Today's digest and `active_model` may
fall back to configuration [E7], so they cannot establish a reported model.

If a report exists, qualify its exact provider/model against the allowlist. Show
configured/reported differences; never substitute the configured value for a
known disallowed report. If no report exists, configured-only evidence counts
**only** under the user's explicit project setting, and only with a concrete
allowed configured ID and no contradictory known fallback. Unknown configuration
or an alias that cannot name an exact model does not qualify. Default the switch
off; the recommendation is to explicitly enable it for initial PTY use. A model
report is attributed evidence, not a cryptographic guarantee.

The bridge rechecks these configured predicates when admitting merge; it does
not select the reviewer or invent workflow policy. Project settings edits bump
affected tasks' `review_version` in the same transaction. No stale approval is
inferred from an old SPA label. Record the admitted facts in a short pending
merge claim. While a Git effect may still run, review/control mutations and
relevant settings changes return `busy`; ordinary unanchored comments continue.
Reconcile the claim before releasing it after a crash. Do not hold AppState
across Git or use a timeout as proof an effect stopped.

### Line and hunk comments

An anchor names revision, old/new path, side, blob OID and inclusive line range;
hunk feedback includes its exact ranges/content digest, not a mutable ordinal.
Derive bounded selected text and context deterministically from the retained
original blob for line matching, using a fixed documented context window.
Validate membership and ranges against retained Git objects. Renames keep both
paths, additions use the new side, deletions the old. Binary/submodule/oversized
content supports file comments with a visible reason line feedback is absent.
Read symlinks as blobs. Current file refs lack immutable review identity [E8].

Never edit the original anchor. On a later revision, identical path/side/blob
keeps placement. Otherwise the SPA may find the original selected content plus
context digest in the same file. Exactly one match is shown as **Moved — best
effort**, linking back to the original; ambiguous/missing matches stay Outdated.
This is a rendering hint, not stored migration or evidence the reviewer saw the
new code. Do not guess across renamed paths in this first implementation.
Missing, truncated or unavailable original/new match material stays Outdated;
do not call a match unique when the whole candidate file has not been searched.
Replies and resolution still refer to the original thread. Drafts live in the
existing `build-ui` store, keyed by task/revision/anchor [E9].

## 2. Wire

Names below are proposals. Use a small typed `tasks.review.*` family, with nine
review verbs and one additional user-only configuration verb. Reuse task CRUD, comments,
assignment and settings in the review response instead of new parallel APIs.

| Method | Contract |
| --- | --- |
| `tasks.review.attach` | Registered source, the user-configured target-binding ID, implementers, expected review version and operation ID. Refuse a second attachment or caller-supplied destination override. |
| `tasks.review.publish` | Expected H and review version; pin revision and open it. No implicit commit. |
| `tasks.review.get` | Consistent current task/review snapshot: revision, decisions/provenance, designation, candidates, structured check comments, operations, project settings and review_version. Include paged task-history continuation using existing task records. |
| `tasks.review.diff`, `.file` | Revision/candidate and bounded body range. Reuse changeset body shapes, immutable content keys, completeness and truncation. `diff` includes the changed-file/commit summary; no separate inventory verbs. |
| `tasks.review.submit` | Exact revision and approve/request_changes/withdraw, rationale, expected review_version, operation ID. Actor/provenance captured by the bridge. |
| `tasks.review.prepare_merge` | Exact revision and expected T/version. Return retained C or conflict facts. |
| `tasks.review.merge` | Exact candidate, expected review_version and operation ID. Service resolves current decisions/check comments; no SPA-generated approval/evidence list. |
| `tasks.review.operation` | Durable operation result after timeout/reconnect/restart. |
| `tasks.review.configure` | Project policy and expected settings version; user-only, never an MCP write. One extra verb is justified because current settings are device-wide [E7], not a typed project-review config API. |

Extend existing `tasks.update` with user-only review-recipient configuration;
`tasks.assign` with optional expected review version and immutable review input;
and `tasks.comment` with optional anchor/reply/resolution or structured check
metadata. These are new, capability-gated fields, not existing behavior [E2].
A check comment contains C, command, exit code or signal, bounded log tail and
producer/time. Its text appears in the same timeline. Thread resolution is a
typed comment action, not a second comment stream or new resolve verb.
Existing task create/list/get/close/reopen retain their normal roles.
The corresponding MCP task tools carry the same review CAS when changing
review-relevant fields; a legacy caller that omits it is refused for those
fields on an attached task. It can still read and post ordinary comments.

Every mutation of review state requires **one** `expected_review_version` CAS.
This includes review-related use of existing task verbs. There are no separate
SPA-computed evidence, thread or step-generation preconditions in release 1.
Project-wide configuration has its own expected settings version; changing it
bumps affected task review versions. Both use the same named `stale_version`
refusal, with expected/actual values and task or project identity. A refused
mutation performs no side effect. Stable operation ID + same actor/body returns
the stored receipt before checking a now-old version; reused ID + different body
is an idempotency conflict. Do not infer this from current INSERT OR IGNORE.
Legacy clients cannot change review control fields without the new capability;
ordinary task comments remain usable.

For an attached task, assignment, status/move, close/reopen, recipient or
contributor edits, anchored/thread/check comments, publish/submit/prepare/merge
all require and bump the review CAS. Its omission is refused even through a
legacy verb. Assignment checks it inside the same transaction as the queued
intent. Ordinary unanchored comments and title/body/label/priority/navigation-link
edits remain outside this CAS and cannot alter review authority or Git inputs.
For an unattached task, existing verbs retain their contract; initial attach
uses the empty review version returned by get. Pure reads need no CAS.

On `stale_version` the SPA preserves the draft, refetches current data, commits
it through the normal version-fenced cache, and repaints. It does not silently
retry approval, assignment or merge against a changed revision. Two tabs can
both render old data; only one can admit a mutation at that version. A later
review mutation makes the other tab refetch. Git target movement separately
returns `target_moved`, and checked-out **local-ref** targets return `target_checked_out`. A remote
branch followed by a checked-out base is supported, not refused.

Reuse the project-scoped `tasks` invalidation [E10]. Bump the review version and
emit only after commit. Read current data independently of timeline paging;
cap decision history and logs without truncating the authoritative current
record. Reuse the existing 32,000-byte task text limit; cap inline log tails at
16 KiB and page larger bodies. Current version fencing must compose with list
paging/read order, not create a second task membership cache [E9].

Baseline wire is 3.5.0 [E1]. Choose the next additive minor at implementation.
Update typed handlers, `api/mod.rs`, versions/hello/events fixtures, verb
fixtures and the prior-minor manifest with `scripts/api-verbs-manifest.mjs`.
Nested safety types reject unknown fields as well as top-level params.
Advertise `tasks.localReview` for attach/publish/get/diff/file/submit/operation,
configure, and the new task comment/update/assignment/CAS fields in increment B.
Advertise `tasks.localReviewMerge` for prepare_merge/merge and structured check
comments only in increment C. Operation lookup is shared; C adds merge outcomes
without removing B's receipt reads. Method names remain individually advertised
by the typed contract. Thus review is usable before merge ships. Cache capability changes;
use the current greeting at dispatch. Old bridges retain normal Tasks/Changes.
No unsafe fallback to a legacy merge verb.

RPC/MCP adapters validate types/scope and call isolated services. The service
persists facts, authenticates actors, checks declared preconditions and performs
Git/delivery. SPA owns review labels, configuration editing and action selection
[E11]. Automatic workflow transition policy is not part of release 1.

## 3. Git mechanics

### Publish and prepare without changing a checkout

The source is the workspace's committed branch. Worktree sources share objects;
Rift sources need local import. Import H into a fresh private review ref, verify
expected H and object connectivity, then publish the revision. Do not use Rift's
current force-publish helper, which updates a shared same-named branch [E6].
This publication uses registered local paths, no network remote or forced shared
ref update, and no dependence on a disposable workspace's object store. Remote
landing, when selected, is a separate explicit merge action. Review reads use retained OIDs.

Release 1 has two explicit target modes. **Build uses remote mode:** the selected
remote's `refs/heads/main` is the authority, normally origin/main. The canonical
repository retains objects and performs the push; its checked-out main follows
the remote and is not the target of the push. **Local-ref mode** remains available
without a remote, but only for an unchecked-out target in the canonical repository.
A local bare repository can serve as a remote without any hosting service.
Reviews, comments and receipts stay in Build; no GitHub PR API is introduced.

The user selects the project target through configure. Agent attach may only
reference that approved binding; it cannot choose another remote/ref or mode.
Once published, its target identity is immutable. The user may restore the same
endpoint identity for recovery, but a new destination requires a new binding and
a new task/review; never retarget a pending push.

Bind a full direct `refs/heads/...` ref and the registered repository. For remote
mode, resolve and validate the selected remote's effective fetch/push endpoint,
including pushurl and URL rewrites. Require a single matching endpoint initially;
refuse multiple push URLs, mirror configuration, configured push options or a
changed endpoint rather than silently pushing elsewhere. Use the verified remote name for read, fetch and push under the operation
reservation, re-resolving and checking the same endpoint identity before each
command. Do not pass an already expanded URL back through Git rewrites: chained
insteadOf rules can otherwise select a second destination. Validate effective
fetch and push URLs, not remote.pushDefault or the project row's display URL. Persist remote name, full ref and a
safe endpoint identity/digest, not a credential-bearing URL. Re-resolve the URL
only if its identity still matches; a changed endpoint leaves recovery uncertain
until restored; selecting a different destination follows the user-only new-task
rule above. The endpoint is derived from the registered
remote, not arbitrary caller input. Redact URLs/credentials from output and errors;
the current `git_failure` includes argv, so do not pass credential-bearing arguments
through it unredacted [E6]. The existing URL validator and unattended runner are seams,
not an already implemented review-push contract.

Read T from that remote ref, fetch it into a fresh private review ref, and verify
the fetched OID equals T before preparing C. A race during fetch requires a new
snapshot. Do not use a possibly stale local main or tracking ref as remote T.
Publication pins its observed T/B for the historical diff; preparation reads the
live remote again. For local-ref mode, read T from the explicit local target.
Reject unborn/deleted or symbolic targets, unrelated histories, non-Git sources,
replaced bindings and source equal to a direct local target. Attaching a review
never creates a target branch. Build's base-sync source must follow the selected
remote/ref under its existing upstream rules; refuse a mismatched configuration
before push instead of asking sync to follow a different destination [E6].

Prepare C from exact T/H with `merge-tree --write-tree` and `commit-tree` with
parents `[T, H]`, including task/revision trailers. Even a fast-forwardable source
gets a merge commit. This preserves the reviewed commits and one clear merge
boundary. Do not offer squash/rebase initially. Persist the candidate metadata
and reuse C for the same inputs rather than changing its timestamp on every
read. Pin C before returning it. Probe installed Git support and cover exact
argument forms in temporary-repository tests. A conflict leaves target untouched;
the implementer fixes the source, commits and publishes a new revision.

Use structured argv, full refs/OIDs, process deadlines and disabled hooks/auto
maintenance, following existing Git process seams [E6]. No external diff or
textconv. Refuse custom merge drivers until explicitly supported. Candidate preparation and the target-ref push/update do not write checkout
files. The separate existing base-sync service keeps its filter and overwrite
guards. Retained blobs,
not arbitrary filesystem reads, supply review content.

### Checks and merge admission

The project agent uses existing assignment to ask a workspace agent to check
exact C in its own checkout. The project agent's usual base cwd is not an
integration workspace [E7]. This plan does not grant permission to write there.
The executing agent records a structured task comment: exact C, command, raw exit
code/signal and log tail. The bridge authenticates the author and matches C and
required command names; it does not observe the process, inspect its cleanliness
or create trusted execution IDs. Show **Reported by <actor>**. Agents should
state checkout divergence in the comment instead of calling it a clean run.

The configured required commands remain data. Use the latest submitted result
for each required command on C; missing/interrupted/nonzero results fail
admission. Keep earlier results visible. Only the assigned check executor named
in the task's handoff input, or the user, may supply a counted check result;
ordinary agents may still comment. Record that assignment's actor. This uses
existing task assignment and a small typed handoff input, not a new runner or
gate scheduler. Validate the producer when the result is appended; handing the
task on for merge does not erase an accepted result on C. The project agent
cannot report another agent's exit as its own.

Merge accepts the candidate plus one expected review_version. In a transaction,
recheck current designated approval, implementer independence, model policy,
check comments on C and task lifecycle; write a pending operation claim. Retain
the admitted comment/decision references for audit, without asking the SPA to
assemble them. If origin/main moves from T, return `target_moved`, fetch the
new T, prepare a new C with parents `[new T, H]`, and run every required check on
that **new C**. Results on the previous C never carry over, even if the patch is
unchanged. Approval of unchanged source H may remain. The same rule applies to
local-ref target movement. Source movement known before admission is reported and
requires explicit publication, never silent inclusion of new commits.

### Land on origin/main with a guarded push

For remote mode, persist the admitted operation before contacting the remote.
Validate that retained C has exactly the admitted parents `[T, H]` and tree.
From the canonical repository, push **only** C to the selected remote's full
ref with the explicit old-OID lease. In Build, the core command is:

```text
git -c core.hooksPath=/dev/null -c gc.auto=0 -c maintenance.auto=false push --porcelain --no-follow-tags --no-mirror --recurse-submodules=no --force-with-lease=refs/heads/main:<T> -- <selected-remote> <C>:refs/heads/main
```

Use structured argv and the existing unattended process/deadline machinery.
Disable client hooks; remote server hooks and branch protection still apply.
C is a child of T, so the proposed change is a fast-forward despite the explicit
lease flag. Never retry with plain force, omit the expected OID, push all refs,
or create another merge commit after checks. No tags, submodules, mirror push
or second endpoint may ride along. Build's own competing operations retain a
short repository/target reservation; remote Git arbitrates other ref writers.
Do not reuse the generic `gitgui::network::push`: it pushes the current branch
and its force mode uses a tracking-based lease, not this exact T/C contract [E6].

A checked-out local main is **not** a reason to refuse this remote push. Do not
update that local branch before or after pushing. A rejected lease is
`target_moved` and requires a fresh candidate/checks. Distinguish authentication,
server-policy and transport failures from a stale target. A deadline or lost
reply can occur after acceptance: keep the outcome uncertain and inspect the
selected remote, not a local tracking ref, before retrying.

On confirmed success, settle a task receipt saying **Pushed C to origin/main**,
with the actual selected target and admitted review/check references. Push only
the selected target, only to the selected remote, only with a lease. For Build,
this push triggers the existing main CI/deploy workflow [E12]; expose that effect
in the merge action. The pending receipt already holds review/check admission
before the push. Do not automatically close the task or delete its workspace.

Recheck the source's upstream still matches the selected target before requesting
the existing `project.sync_source`; a mismatch is a separate sync refusal after
integration, never a reason to push elsewhere. That RPC
returns `pending: true`; its answer is not proof the checkout caught up
(`app/projects/base_sync.rs:482–502`). The service takes SyncLock, fetches the
configured upstream, and when local main is behind it runs the guarded
`fast_forward_checkout` path [E6]. Dirty or in-progress checkouts, divergence,
filters, timeouts and an upstream that advanced again remain the sync service's
reported outcomes. It may sync to a later descendant, not necessarily exactly C.
It never needs a preparatory local update-ref from the review service.

Keep a sync-request pending indication and the source's last service status as
separate cache-backed information. Current SyncStatus has state/reason/timestamps
and counts, **no endpoint/ref/OID or request identity**, and a later service pass
can overwrite it [E6]. Label it Last base sync; it is not proof that C was checked
out or pushed. A successful push stays integrated even if local sync is blocked.
Save a small sync-request pending bit with the merge result. Do not clear it
when the RPC returns: that could lose an in-memory request at restart. When the
existing service starts a pass, snapshot the pending review-operation IDs for
that source; when that pass settles, atomically record its attempt outcome and
clear only those captured bits with the source status. This small internal
acknowledgment is new integration work, not a current RPC guarantee. A skipped
or failed attempt is reported, not successful sync. An older pass cannot clear
a newer request merely because its timestamp is later. On restart, nudge any
still-owed request again; duplicate sync requests are safe and never repeat the
push. Explicit user cancellation may clear an owed request. This adds no wire
verb or second checkout scheduler. Local sync does not count as deployment
verification.

### No remote: direct local-ref landing

For local-ref mode, keep the earlier unchecked-out-only rule. Under a short
repository/ref reservation, enumerate `git worktree list --porcelain -z`, refuse
a target checked out anywhere, and update it with `git update-ref <ref> C T`.
Also refuse a target managed by automatic base sync in this mode. Coordinate
this narrow placement/removal reservation with Build checkout/branch creation
and source configuration changes; external shells remain outside that ownership.
Never write the checked-out ref and hope later sync repairs its index/files.
The previous disposable experiment remains valid for that rejected local path.
A local bare remote can instead use the guarded-push path above.

### Small recovery record

C is prepared and pinned before merge. Keep one pending merge intent and a
settled outcome. After restart, prove any old Git child has stopped or finished
before retrying; a timer alone is not proof. For remote mode query the **selected
endpoint/ref** with `ls-remote`, under a deadline. An unreachable endpoint is
unknown, not T. For local-ref mode read that target directly.

| Actual target after interruption | Result |
| --- | --- |
| Exactly C | Verify retained C's parents/tree and settle integration once; mark it reconciled if the push reply was lost. Queue missing base-sync request separately. |
| Exactly T | No current integration observed. After an unknown network outcome, keep the attempt uncertain: it could have landed and been rolled back, with server hooks already fired. An explicit retry requires fresh binding/admission/check validation on C and the same T lease; never auto-replay on equality alone. Local-ref mode also rechecks checkout placement. |
| Another OID | Report target_moved/uncertain. Fetch the observed remote OID privately when needed to prove ancestry. If it contains exact C, reconcile integration without pushing again; if only H is present, record external integration. Otherwise prepare a new C and run new checks. |
| Ref absent, endpoint changed or unreachable | Do not create a branch, change destination or infer success. Report missing/configuration failure or uncertain transport and wait for an explicit retry/rebinding decision. |

The remote may move again between ls-remote and fetch: accept an ancestry proof
only for the OID actually fetched/verified, otherwise re-read. Retained C and a
pending intent establish recoverable integration facts, not proof of which
actor pushed when its response was lost. A force move away and back cannot be
reconstructed from OIDs alone. Never reset or blindly replay. `.operation` is
the readback after a timeout, not a new merge request.

Fault-inject after remote acceptance/before reply, after target change/before
SQLite, and after SQLite/before the sync nudge. Test moved origin with fresh C
and mandatory fresh checks, offline recovery, server rejection, dirty/in-progress
base sync, and exact lease behavior. Also test that a poisoned tracking ref or local base
containing C cannot settle a remote receipt, and that endpoint mismatch refuses
the action without leaking credentials. There is no new index/worktree crash repair
in review landing; the existing source-sync service owns its checkout operation.
Do not reuse `classify_stage_publication` as remote evidence: today it can return
Merged from local base reachability alone [E6]. Neither local main, a tracking
ref, nor a successful sync response can settle the remote merge record.

### Roll audit and workspace retention

Keep workspaces and source branches after merge. Current reclaim/deletion guards
still apply [E4, E5]; do not add `workspace.cleanup_integrated` or pretend pins
protect unpublished follow-on work. New local integration cleanup belongs in a
later lifecycle slice. Review attachments suppress the automatic linked-task
close hook, as section 6 specifies, but do not authorize deletion.

Release 1 has **no** per-tier deployment baseline, coverage gate, user range
approval or CI enforcement. Current CI responds to main pushes/manual dispatch
and does not read local review state [E12]. Keep mandatory independent review
in the project-agent roll procedure, and record verification on the task. The
feature cannot prove all deployed work was covered or prevent shell bypass.

An optional read-only roll report can compare an explicitly selected tag to an
exact proposed SHA, walk first-parent merge commits, and show which lack task
merge receipts. Use existing Git reads plus review receipts; no new coverage
verb is needed. Refuse a non-ancestor/missing tag and show the full chosen range.
Do not infer a trusted deployed baseline from the newest tag. Direct commits
must be shown separately as unclassified; a merge-only report cannot certify
them. This report is a small follow-on, not a release-1 dependency or exemption.

## 4. SPA

Use the existing tracker task route
`#/device/<deviceId>/project/<projectId>/tasks/<taskId>` and a **Changes** section.
Device-less links resolve to it. Preserve `/c/<commentId>` focus and add revision
selection without a new PR collection [E8]. Tasks may filter by review state;
workspace Changes links to its owning task. The first release has no attachment
picker because there can be only one attachment.

Show the branch/target, revision/commits/files, anchored feedback, designated
reviewer, model provenance, reported checks and precise merge result. Actions
are attach, publish, assign for review, approve/request changes, prepare and
merge. Candidate checks/deployment are existing agent handoffs. Human review
names Zech as recipient and uses In review; agent review stays In progress.
For Build, label the action **Merge and push to origin/main**, show that it
starts the existing CI/deploy workflow, and show base-sync status separately
from the push receipt and deployment verification. Only local-ref mode refuses
a checked-out target; the bridge rechecks the selected mode at admission.

Reuse `changesReview.js`, `changesComments.js`, `changesetBodies.js` and the diff
viewport/folds/composer via a focused task-comment adapter. Current file viewed
marks and `run.request_changes` conversation posts are not task approval [E8].
Keep historical `taskReview.js` plan browsing separate. Render the same comment
record in timeline and diff, including original and best-effort Moved links.

Add focused task-review cache/sync/model/action modules beside tracker modules.
Current record address is device/project/task; review_version lives in its value
and write fence. Immutable bodies use repository/revision/blob/path/options.
Reuse body paging, content-key fences, task list read ordering, subscription
repair and separate UI drafts [E9]. Do not evict task history with its workspace.
Distinguish missing, empty, truncated and unavailable.

Task pushes carry IDs, not review content [E10]. An invalidation or partial
mutation receipt triggers a canonical refresh; a complete returned snapshot may
be written directly to the cache. `tasks.review.get` reads the current task,
review block and requested timeline page coherently. Write timeline comments
and their anchor metadata together under that review version. If composing
separate task/review reads, require matching review versions; reject and reread
mixed versions instead of painting mismatched comments. All paint comes from
the version-fenced cache after writes, never directly from a reply. Cold or
offline mounts draw held data. Do not use a private in-memory store when cache
recovery is pending. `stale_version` preserves
the draft and triggers a versioned refetch/repaint, then asks for a fresh action.
The same rule applies across two tabs. Approval/merge is pending until admitted;
optimistic comments use operation IDs and the normal task comment path.

Tests cover real cache-write-to-redraw, concurrent tabs, late invalidation,
comment deep links, body continuation, repeated-line ambiguity, moved versus
outdated anchors, capability fallback, keyboard navigation and a 390px layout.

## 5. Agents and workspace handoffs

### First PR: fix assignment durability for all callers

Today `assign_task_to` calls `deliver_for` before `settle_assignment` and
`commit_task_write` (`dispatch.rs:259–270`); new-workspace dispatch has a similar
boundary [E2]. A reviewer can start before its assignment is durable. Fix this
once for wire, MCP, existing/new-agent and new-workspace callers. Do **not** keep
a second legacy ordering for tasks without reviews.

Split preparation from delivery. Resolve/create the destination identity and,
where needed, provision a workspace without delivering its task. Then commit
assignment, task events/links, input revision, destination conversation transcript
and owner/agent records, and queued delivery receipt with its operation ID in
**one SQLite transaction**, extending the existing store composites [E3].
Only after commit may the delivery runner claim and enqueue/provider-deliver it.
A failed commit leaves no runnable task delivery; a crash after commit is resumed
from the outbox. Reuse the current delivery receipt/claim machinery [E3, E7],
with a durable link from assignment intent to the delivery transcript/receipt.
Do not make two independent commits masquerade as an atomic handoff.

Split tracker persistence from after-commit notices and in-memory state. In
particular, set `dispatched_task` only after commit, and reconstruct it from the
durable assignment payload when a queued turn resumes. Today dropping the
deferred in-memory queue does not remove an already persisted delivery receipt;
boot recovery can replay it [E3]. Validate older queued assignment receipts
against the task's durable recipient and dispatched operation before replay.
An absent/mismatched assignment becomes uncertain without delivery. A reused
operation ID is successful only if both the receipt and assignment/event match;
the current receipt-only early return is insufficient for pre-fix orphans.

Filesystem creation cannot join SQLite. Keep a preparation receipt; an
interrupted provision may leave an idle reusable workspace, never a recipient
already executing the uncommitted task. Recheck expected assignment before the
commit. A superseding assignment cancels an unclaimed old intent; provider
acceptance that is uncertain is reported, not blindly redelivered. Persist
recipient/input identity in the payload so a resumed recipient reads the current
task before acting. Do not promise exactly-once execution across provider
failure. Ordinary coalesced comment notices are not being rebuilt as an outbox.

This fix is independently useful and must ship before review-specific work.
Tests inject failures before/after task commit and provider claim, plus concurrent
reassignment and old orphan receipts, and cover **both review and legacy callers**,
including create-with-assignee. User/unassigned targets have no provider receipt
and keep a normal task transaction. The legacy repair need not add a client
retry-deduplication promise: review-capable assignment later adds caller-stable
operation IDs alongside its CAS. No browser is required
to resume a committed delivery.

### Release 1 uses explicit assignment, not an automatic sequence

Keep `get_task`, `comment_task`, `assign_task`, `move_task`, `link_task` and existing
workspace/agent creation tools. Add MCP counterparts for the nine review verbs,
not separate workflow, cleanup, coverage or settings tools. User-only recipient
and project policy edits have no agent write counterpart. Scope every task,
repository and child ID to the authenticated conversation. Coding agents publish,
read and submit; the Project agent prepares/merges. Checks run under the workspace
agent that actually executes them, using structured `comment_task` evidence.

1. The implementer commits, attaches and publishes H, then asks through the task
   for the already user-designated recipient to review. If none is designated,
   that is an explicit user setup action, not permission to pick a puppet.
2. The controller uses existing workspace/agent provisioning and `assign_task`.
   The reviewer imports retained H into its own workspace and checks it out
   detached or on its own review branch, then verifies HEAD. No product-owned
   checkout-transfer state machine ships here. Never run ad hoc `git worktree
   add` on a registered repository or change the implementer's checkout.
3. The reviewer reads that revision and posts anchored findings plus one decisive
   submission. On requested changes, the controller explicitly assigns the task
   back to its saved implementer. Fixes publish a new revision and need re-review.
4. On approval, explicit assignment hands checks to an executor, then integration
   to the project agent. Each assignment carries its immutable H or C input and
   expected review version. The user can be the reviewer or an explicitly chosen
   handoff recipient; no other step implicitly waits on them.
5. The project agent prepares/checks C, pushes it to the selected origin/main
   with the lease, requests base sync, and follows the existing roll procedure.
   It records deployment verification and explicitly moves/closes the task. A source/target
   race sends it back for new preparation or review; Complete is never approval.

There is no automatic submit→assign transition in release 1. If the controller
stops between those actions, the task shows a durable decision awaiting handoff;
it does not secretly lose a queued transition. Existing assignment delivery is
headless after it is accepted. #145 owns the later automatic sequence and branch
checkout transfer; the task identity and handoff convention are adopted now.

For that follow-on, workflow is user-authored data: ordered steps with recipient,
kind and explicit `on_decision` edges. An isolated bridge service may apply only
the declared transition to `step[n+1]` or `step[n-1]`, under expected-generation
CAS, and durably queue delivery. It must not choose a reviewer, infer a gate,
interpret an ordinary comment as a decision, or invent a next step. The SPA edits
and displays the data. Persist decision plus transition intent together when
that automatic feature is added. This limit follows rule 5 [E11]; only delivery
and accepted transition execution need to continue without a browser.

Update `bridge/templates/notes/{task_tools,workspace}.md`,
`bridge/templates/project_agent.md`, `bridge/src/templates.rs`, MCP descriptions
and `bridge/src/orchestrator/workspace.rs` together [E7]. Teach exact revisions,
designated review, task comments, check provenance, manual handoff and explicit
roll verification. Post only decisions, necessary questions, blockers and changed
state; no acknowledgments or repeated diff recaps. Capability-gate instructions.
Templates are compiled defaults, not current per-project overrides.

## 6. Tasks integration (the original Issues integration section)

**A task has opted in if it has a branch attachment.** Test the stored child row,
not a prompt, transient dispatch flag, separate workflow table or current board
column. The attachment is retained after merge/close, so the opt-in remains
stable. Existing file uploads and `links.branches` do not opt a task in.

For these tasks, suppress `activity.rs`'s Complete→In review movement and
`workspaces/deletion.rs`'s merged-Finish linked-task close [E2, E4]. Preserve the
Complete report and merge history, but wait for explicit task actions to change
completion. Non-review tasks keep current behavior. Test both paths, including a
workspace linked to one review task and one ordinary task. Do not let cleanup
close the review task merely because it shares that workspace.

The task remains In progress for agent review and check work. A handoff to Zech
explicitly assigns the user and moves it to In review through existing tools;
Needs you uses current assignment/mention/read-mark behavior. This follows #89
and #145; opening a diff or leaving an ordinary comment does not request a human.

For Build, the merge receipt now proves integration into origin/main, resolving
the earlier side-branch/completion gap. That push starts current CI, but neither
push success nor base sync means deployment passed. The deploy agent records
the verified release SHA (C or a release containing C), outcome and verification
evidence, then explicitly moves Done and closes. A failed deployment keeps the
task open with its merge receipt intact. This is release-1 task evidence and
the project-agent procedure, followed by explicit CAS move/close actions; the
bridge does not verify deployment or enforce a typed deployment gate. Projects
without deployment may do this after verified merge. Closure and the board column remain
separate actions. Task close/reopen must carry the review CAS when relevant;
closing refuses new merge admission, but cannot undo an admitted Git effect.
A reopening never silently replays merge/deployment. Related tasks get no
approval or completion just from being linked.

This planning task still moves to In review when its document is ready for
Zech. The plan does not opt itself into a feature that has not been built.

## 7. Rollout

**Release 1: 11–17 focused engineer days**, including review and failure-path
tests. This estimate is for the narrow path above, with one repository/attachment,
explicit handoffs, guarded remote push and local-ref fallback. The remote path
adds 1–2 days to increment C; this is not an estimate for all of #145 or for
production deployment coverage. If a prerequisite exceeds its
budget, re-estimate openly; do not silently drop durability or widen scope.

| Increment | Files and deliverable | Tests and rollout | Estimate |
| --- | --- | --- | --- |
| A: assignment fix | `bridge/src/app/tracker/dispatch.rs`, tracker/store transaction seams, delivery receipt/runner integration. Prepare destination, commit assignment + queued intent, deliver after commit for every caller. | Commit/claim/restart faults, superseding assignment, wire/MCP and new-workspace paths. Bridge roll; ships alone and benefits existing tasks. | 2–3 days |
| B: usable review | Proposed `bridge/src/task_reviews/`, `store/task_reviews.rs`, `api/v1/task_reviews.rs` and thin app adapter; extend tracker/schema/MCP/fixtures. One attachment, pin/publish, anchors, designation/provenance and decisions; activity/deletion attachment opt-in guards. Task Changes adapter in `spa/src/core/`, extend tracker page/render/timeline/cache/router and `spa/src/views/trackerTaskView.js`. | Migration/pins/restart/GC, actor/scope/CAS, configured-only toggle, non-designated/self approval, moved/outdated anchors, two-tab cache, mobile and legacy hook both paths. Coordinated bridge+app roll with review capability. Users can review and hand off before merge ships. | 5–7 days |
| C: prepare and merge | Candidate/local-ref and guarded remote-push recovery in task-review Git service; structured check comments; reuse `project.sync_source`/source status with durable nudge acknowledgment and unattended Git; narrow reservations, MCP/templates and merge UI. | Temporary worktree/Rift/bare-remote tests: exact C/lease, moved origin needs new checks, single endpoint/ref including URL rewrites, lost push reply, offline recovery, crash before sync pass/ack, sync dirty/in-progress refusal, separate push/sync/deploy state, local-ref checked-out refusal and self-merge refusal. No live project push needed. Bridge+app merge capability; amend architecture/specs. | 4–7 days |

Do not hold increment A for the final capability. Do not announce merge before
its tests pass. Increment B supports the existing manual roll workflow while C
is built. Activation requires an owner-selected target mode/remote/ref (origin/main for
Build), a matching base-sync source, designated reviewer, explicit model settings
and gate commands. These choices
are part of the estimate, not hidden automatic discovery work.

Follow-ons are separate tasks/estimates, not release-1 gates:

| Later slice | Boundary / likely size |
| --- | --- |
| #145 sequence and checkout transfer | User-authored step data, bounded transition CAS, exact-revision workspace preparation/recovery. Bridge+app, 5–8 days before richer editing. |
| Multiple attachments | Introduce round table, whole-set decisions and partial-integration semantics only when needed. Bridge+app, 3–5 days. |
| Direct checked-out local landing without a remote | Still deferred. Remote push plus existing base sync already supports Build main; writing a checked-out local ref directly needs its own ownership/recovery design. Bridge+app, 4–7 days. |
| Read-only roll audit | Selected tag→SHA first-parent report against receipts, direct commits labeled. Existing reads/procedure, 1–2 days. |
| Coverage/range approval/cleanup | Separate product and trust decisions, then estimates. No promised per-tier baseline, CI guarantee or duplicate reclaim system. |

Implement with TDD and the relevant AGENTS.md gates: Rust tests/fmt/clippy, SPA
lint/full Vitest/build where touched, semgrep, gitleaks and diff-check, judged by
exit status under `nice -n 10`. Add the review security checklist during
implementation for scope, actor/model evidence, anchors, process inputs and
transaction boundaries. This plan claims no product tests or checklist passes.
Use disposable repositories and isolated test checkouts, never the user's base
checkout, bridge binary, paired client or browser to validate this plan.

## 8. Open questions for Zech

The task identity, one discussion stream, independent review and explicit human
step type are settled by #89/#145. The following choices have recommendations;
record the owner's answers before feature activation, not before finishing this
planning document.

| Choice | Recommendation |
| --- | --- |
| First-release scope | One attachment, manual assignment, diff/comments/review and guarded push to main, with unchecked-out local-ref fallback. Keep #145 automation and multi-branch scope later. |
| Target for the first rollout | Push to the selected origin/main with an explicit lease; request existing base sync afterward. Local-only projects use an unchecked-out ref or a local bare remote. Estimate is now 11–17 days. |
| Review recipient and model list | User designates one concrete reviewer (or themselves), selects exact provider/model IDs, and keeps agent reviewer separate from merger. No September sample IDs as automatic defaults. |
| Configured-only model evidence | Explicitly enable it for initial PTY use if acceptable; show its source. The implementation default is off until chosen. A known disallowed reported model never falls back to the configured one. |
| Check evidence | Actor-reported structured task comments on exact C, with required commands from configuration. No claim of bridge-observed execution. |
| Who may merge? | Project agent or user. An agent never merges using its own approval. |
| Merge and hooks | No-ff only; client hooks disabled for the primitive, explicit checks on exact C. Remote hooks/branch protection still apply. |
| State and completion | Task Changes surface and Review actions; agent work stays In progress. Build moves Done/closes after recorded deployment verification; retain branches/workspaces. |
| Future roll guarantees | Begin with an optional read-only report. Plan per-tier coverage/enforcement only with a real baseline and deployment integration. |

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

**Refresh draft (2026-10-02, `700e642e`):** reapplied the original plan before
updating it against `adf08b06`, preserving its history. Read #89/#145 timelines
and #86's later always-on-service correction. Corrected task/wire/cache/lifecycle
claims, then proposed a 30–48-day slice with rounds, workflow, coverage and
cleanup. Opus 5.5 confirmed the cited current-code claims but challenged scope
and placement of logic. That large first-release scope is now superseded below.

**October challenge round 1 (2026-10-02, Opus 5.5 via project agent, on 700e642e):** decisions by
the Astra lead on the 15 numbered challenges. “Accept” accepts the stated core
change; any narrower alternative or retained boundary is explicit. These are the
lead's decisions at 49aa3469; landing and summary are superseded by round 2 below.

| # | Decision and reason | Result at 49aa3469 |
| --- | --- | --- |
| 1 | **Accept.** The 30–48-day all-at-once feature delayed useful review behind automation and deployment machinery. | Three independently usable increments, 10–15 days total. Existing assignment/agent gates; unchecked-out CAS landing only. Limitation for checked-out main is on the first page. |
| 2 | **Accept.** One attachment removes artificial whole-set rounds and partial-merge successors. Build's current task use does not justify that machinery in release 1, while #145 still owns future multi-branch intent. | Revision is the round; no round table. Additional repositories use separate tasks until the later migration. |
| 3 | **Accept, with automation deferred.** The bridge must not decide product policy from a submission. Existing accepted delivery already needs to run headless. | Manual `assign_task` now. Future ordered step data permits only declared on_decision forward/back edges with generation CAS, never inferred policy. |
| 4 | **Accept.** Delivery-before-persistence is an existing correctness fault; preserving it for legacy callers would create two inconsistent assignment paths. | First small PR persists assignment plus queued delivery intent together and delivers after commit for all callers, including deferred new-workspace assignment. |
| 5 | **Accept.** Reported model information can be absent or parsed; configured fallback must stay visible without making the entire PTY workflow unusable. | Configured/reported/source capture; explicit user configured-only switch. A known disallowed report cannot be hidden behind an allowed configured value. |
| 6 | **Accept.** A model allowlist alone lets an implementer nominate a convenient reviewer and lets an agent merge its own decision. | Only the user-designated current review recipient submits a decisive approval. Record designation and dispatch actors; keep implementer identity. Agent reviewer and merger must differ. |
| 7 | **Accept deferring general reservations; reject both checked-out-target shortcuts for release 1.** SyncLock excludes sync calls, not all checkout writers. CAS on checked-out main moves HEAD without index/files; source sync is not a repair mechanism. | Refuse all checked-out targets. Keep only narrow ref/placement/removal coordination. The disposable Git experiment and current sync early return support this decision. |
| 8 | **Accept.** Actor-attested checks gain no process trust from invented execution IDs or cleanliness snapshots. | Structured task comment on C, command, raw exit/signal and log tail. Match required commands/exact C and authenticated producer; label the evidence reported. |
| 9 | **Accept.** No per-tier baseline or local-store CI enforcement exists, and cleanup duplicates lifecycle work. | Remove coverage/range approval/cleanup from release 1. Optional tag→SHA audit later, explicitly incomplete for direct commits/deployment state. Mandatory review remains a roll procedure obligation. |
| 10 | **Accept.** Identical-blob-only display makes useful comments disappear after nearby edits. | Immutable original plus unique content/context match in the same file, labeled Moved; no stored migration, and ambiguity stays Outdated. |
| 11 | **Accept reduction; retain one explicit config write.** Nine review verbs cover the requested path. User-only project policy needs an honest contract; today's device settings are not that API. | Nine review verbs plus configure; existing task CRUD/assignment/comments, no workflow/coverage/cleanup inventories. Read config with current review data. |
| 12 | **Accept.** The client should not assemble authority from stale evidence IDs. | One review_version CAS per task-review mutation; named stale_version refusal, cache refetch/repaint and explicit fresh action. Project settings have their own CAS and bump affected task versions. |
| 13 | **Accept.** Opt-in must be durable and unambiguous at both legacy hooks. | Existence of the branch attachment is the opt-in; keep attachment history after merge/close. Test review and ordinary tasks together. |
| 14 | **Accept simplification; reject an exhaustive two-case recovery model.** Ref-only landing removes checkout phases, but a third writer can move the target after our CAS. | Pending/settled intent, C/T checks, and an explicit other-OID/uncertain path. Retain C ancestry for reconciliation; never reset or blindly replay. |
| 15 | **Accept.** The owner should be able to judge scope and choices without reading the storage design. | One-page scope, limit, estimate and owner recommendations precede the technical sections; remove superseded mechanisms instead of hedging them as release-1 options. |

Retained boundaries after this round: an explicit configured reviewer rule,
immutable approval scope, truthful check/model provenance, no-ff candidate and
small Git/SQLite recovery record. Removed first-release obligations are deferred,
not represented as implemented or waived review requirements. This round's
summary and open choices were posted on #89 before the round-2 challenge.

**Main advanced during round-1 validation:** #322 landed at `2938697c`, moving
wire 3.4.0 to 3.5.0 and correcting the architecture version summary. Its changed
files and relevant architecture/API/main/SPA wiring diffs were inspected; the
review, assignment, Git and cache contracts cited here are otherwise unchanged.
The document branch was rebased and current-version references updated. The
Opus challenge still refers to its original 700e642e/adf08b06 baseline.

**October challenge round 2 (2026-10-02, Opus 5.5, on 49aa3469):** Opus said
“signoff: no” pending four changes. It accepted the earlier #11 configuration
verb and #14 other-OID recovery qualifications. The lead accepts all four new
challenges. Final signoff is pending the arranged read of this pushed revision.

| # | Lead decision and reason | Result |
| --- | --- | --- |
| 1 | **Accept.** The earlier experiment proved that local update-ref cannot safely move checked-out main; it did not rule out a guarded push to the remote authority. A side branch would not finish Build's real merge workflow. | Push exact C to selected origin/main under an explicit T lease, then request existing project.sync_source. Base sync is separate and can be blocked without undoing remote integration. Keep unchecked-out-only for direct local refs. Use a user-selected endpoint/ref, distinguish transport uncertainty, retain other-OID recovery and acknowledge the separate sync request durably. Increment C adds 1–2 days; total is 11–17. |
| 2 | **Accept.** Done-after-deployment only fits this project when reviewed C actually reaches its main. | The receipt names origin/main; the deploy agent verifies the actual release containing C before Done/closure. Push, base sync and deployment are separate outcomes. Main push triggers current CI. |
| 3 | **Accept.** Changed T makes a different merge commit, so checks of the old C cannot cover it. | A moved origin returns target_moved, then requires new T/C and every required check again on the new C. Source approval may remain for unchanged H. |
| 4 | **Accept.** The owner summary should present the usable path and choices in plain words. | About 15 lines, main push/sync recommendation, explicit project-agent/user merge authority and no agent self-merge; plain configured-model choice. Audit detail is below the summary. |

Validation of the new Git path used disposable repositories only: pushing C with
an explicit lease left checked-out local main at T; fetch plus guarded ff-only
then updated it cleanly. A concurrent remote advance refused the stale candidate
and preserved both the other writer and the local checkout. This did not execute
a bridge or access the project base checkout. Code review confirms the sync RPC
only queues work; its source row carries the eventual result [E6].

The evidence index records current code, not proposed module existence. All
paths are repository-relative at current `origin/main` baseline
`caadc027fb2f4529c15b748782d5afb3852037c1`, verified 2026-10-02 UTC.
Main advanced from 2938697c during round 2 only by a desktop-installer test change;
that diff was inspected and the review-code evidence is unchanged.
Sections 1–8 are proposed implementation, except their identified current behavior.
Read `AGENTS.md` and `ARCHITECTURE.md` before implementation. The listed existing
symbols are the implementation seams; new files in section 7 are proposals.

| Ref | Current source and fact |
| --- | --- |
| E1 | `bridge/src/api/mod.rs:37` (`API_VERSION = 3.5.0`), `fixtures/api/versions.json`; `api/v1/mod.rs` (`parse_params`, typed dispatch); `api/v1/tasks.rs` (tracker verbs); `scripts/api-verbs-manifest.mjs`, `bridge/tests/api_contract.rs`, `spa/test/apiContract.test.js`. Wire names/strictness/capability/version discipline; `api/v1/` is not the wire major. |
| E2 | `bridge/src/tracker.rs:213,275,477` (TaskLinks, Task, TaskComment); `bridge/src/app/tracker/{mod,views,pages,dispatch,activity,notices,tools,attachments}.rs`; `bridge/src/store/tracker.rs:144` (`save_tracker_task_activity`). Existing canonical task/timeline, path-scoped storage, uploaded attachments, paged list, assignment/notice and Complete behavior. |
| E3 | `bridge/src/store.rs` (Store/open), `bridge/src/store/schema.rs`, `bridge/src/store/operations.rs:26` (`backup_to`); `bridge/src/app/conversations/operation_ledger.rs`, `bridge/src/app/runtime/delivery/receipts.rs`. `bridge/src/app/{transactions,runtime/deferred,runtime/recovery}.rs`, `bridge/src/app/conversations/post.rs`, `bridge/src/store/entities.rs`. Owner/transcript/receipt composites exist separately from tracker writes; queued operations recover at boot. No current atomic task-assignment/review transaction is implied. |
| E4 | `bridge/src/app/workspaces/mod.rs:544` (`workspace_create`), `:648` (`finish_workspace`), `:808` (`workspace_finish_legacy`); `app/workspaces/deletion.rs:314` (`remove_workspace`, closure only for merged Finish); `workspace.rs:1004` (`summary_finish_blockers`). Current creation, Finish deletion support and remote-publication completion guards. |
| E5 | `bridge/src/app/workspaces/reclaim.rs` (reservations), `app/workspaces/reclaim/explicit.rs`, `reclaim/containment.rs`, `app/workspaces/branch_delete/{mod,checkouts,defaults}.rs`; `gitgui/unpushed.rs`. Managed-path safety, explicit reclaim and conditional branch deletion already exist; their publication proof is remote-based. |
| E6 | `bridge/src/app/projects/mod.rs:8` (`ProjectSource`), `:25`/`:29` (sync defaults), `app/projects/base_sync.rs`; `source_sync.rs:231` (`run`, early UpToDate), `:309` (`fast_forward`), `:424` (`SyncLock::acquire`), `source_sync/{checkout,in_progress,git,upstream}.rs`, `remote_url.rs`, `git_process.rs`, `gitgui/network.rs:106`, `lifecycle/publication.rs:81`. `app/projects/base_sync.rs:482` queues project.sync_source and returns pending; the service fetches and applies guarded ff-only, then persists source status. Its upstream follows branch remote/merge configuration or documented defaults. Current SyncStatus lacks endpoint/ref/OID and request identity; locks are sync-path-specific. Generic push uses the current branch/tracking lease, and publication classification can accept local base reachability. New review endpoint binding/lease/recovery behavior is proposed, not already implemented. `isolation/rift.rs:396,509` force-fetches shared branch refs; `isolation/worktree.rs:58` needs no publication because objects are shared. |
| E7 | `bridge/src/mcp.rs` (`BridgeAction`, tool lists/surfaces); `bridge/src/app/{mcp,tracker/tools,runtime/spawning,projects/conversation}.rs`; `bridge/src/{delivery,agent,templates}.rs`; `bridge/src/app/runtime/agents/{records,endpoints}.rs` (observed model versus digest fallback), `bridge/src/app/runtime/sessions/registry.rs` (generation), `bridge/src/harness/installed/`; `bridge/templates/notes/{task_tools,workspace}.md`, `bridge/templates/project_agent.md`, `bridge/src/orchestrator/workspace.rs`. `bridge/src/app/config/settings.rs` (`settings_get`/`settings_set` are device-wide). MCP authority, assignment delivery, model/prompt/settings seams. |
| E8 | `spa/src/core/router.js:145,492`, `spa/src/app.js:296`, `spa/src/views/trackerTaskView.js`, `spa/src/core/{trackerTaskPage,trackerTaskRender,trackerTimeline}.js`; `spa/src/views/{taskReview,worktreeReview,workspaceChanges}.js`, `spa/src/core/{changesReview,changesComments,changesetBodies,diffRender}.js`; `bridge/src/thread/items.rs` (`ThreadLink`), `bridge/src/app/tracker/refs.rs`. Tracker versus legacy review routes; existing diff/composer stack and mutable file refs. |
| E9 | `spa/src/core/{localCache,localUiState,localUiStore,cacheSync,pushFence,taskReadOrder,bodyPages,cachedBodies,surfaceContext,deviceContexts}.js`; `core/bridgeApi/{index,v1/index}.js`, `core/changeEvents.js`. Cache-only paint, separate drafts, body paging, subscription repair, read ordering and greeting/capability gates. |
| E10 | `bridge/src/changes.rs` (`Kind::Tasks`, task invalidations/bounds) and `spa/src/core/cacheSync.js` task appliers. Task push is an invalidation, not a durable review/event replay service. |
| E11 | `AGENTS.md` design rules 5–6 and `ARCHITECTURE.md` RPC/push and persistence sections; #86/c/tc-01M37YEQS990896PCGFJV7KYGA. Client-safe policy/paint in SPA; autonomous work in isolated bridge services. Existing request handlers still contain policy and need extraction rather than being assumed thin. |
| E12 | `.github/workflows/ci.yml:14` (main push/manual dispatch) and tier filtering/roll jobs. Build deployment is not an automatic local review-store consumer; deployment coverage is deferred, not a release-1 gate. |

Historical specs read for intent: `planning/v2/Tasks Spec.md` (formerly Issues),
`Bridge Wire Protocol Spec.md`, `E2EE Platform Scope.md`, `UI Design Brief for E2EE Platform.md`. Their retired scheduler, old wire versions or removed methods
are not current implementation authority. This refresh changes only this plan.
