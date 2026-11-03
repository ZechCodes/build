# Workspace reviews

## Snapshot retirement (#974)

Wire 3.31.0 announces `tasks.review.snapshotRetired`. Workspace review opening is
the only creation flow; old `snapshot` reviews remain read-only with no bulk
conversion. Compact task summaries carry optional `legacy_review: true`, and the
SPA shows Legacy review chips on list, board and inbox rows even on a cold cache.
Cached legacy metadata remains a fallback for older bridges.

Legacy reviews use the saved Changes/Files template with the notice
“Legacy review (read-only)” and no verdict bar. “Request a review from @workspace”
opens the workspace form with `for_task_id` preset to the legacy task.
More retains Close legacy review, backed by the legacy-only
`tasks.review.complete` RPC. The former Mark complete form and Merge & Push
panel are removed. Shared `tasks.review.get` / `get_review` and
`tasks.review.diff` / `read_review` keep saved revisions and action facts readable.
Anchored/inline review comments, the pending tray and verdicts are read-only;
the ordinary task timeline still allows plain comments.

`tasks.review.snapshot` refuses new or updated legacy snapshots with structured,
nonretryable `invalid_params` and the instruction
“Request a review from the workspace (tasks.review.open)”. Its compatibility
path on an existing PR still refreshes that review. `tasks.review.act` refuses
legacy mode too. Agents have no legacy capture, selected-action or completion
tool. PR lifecycle tools remain available.

Opening a review for a task with an open legacy review completes that old review
with “Superseded by #N”, naming the new review task, and one ReviewCompleted
event. The work task keeps its status, done_at and trackers. The completion, parent link,
new task and first revision publish in one transaction. Retrying the original
opening reuses its identity without duplicating either review or completion.

Run/worktree branch views hide their legacy Merge and abandon controls when the
workspace has a review. Runs with no review retain those controls as their
completion path. The live workspace Changes tray is Notes to agent, distinct
from anchored comments on saved review revisions.

## Workspace-only creation (#971)

Request review in the workspace Changes header, workspace menu and toolbar opens
one form. The form retains title, description, per-directory bases and exclusions.
Reviewer defaults to You and can select an existing project or workspace agent.
For task is optional, lists open work tasks (including legacy reviews) in the
project and preselects an eligible task linked to the workspace. It is gated independently by
`tasks.review.forTask`; a bridge without `tasks.review.pullRequests` shows
“Update the bridge to request reviews”. Once published, the entry reads View review.

`tasks.review.open` / `open_review` accept `for_task_id`. The journal retains the
selection and publication validates that the target is still open, in the same
project and eligible for a work-task link, before atomically publishing the review
task with `parent_task_id` set to that target. The original request remains in the
opening journal for retries. The current task's `links.parent_task_id` is the
relationship's source of truth, and review summaries and workspace `active_review`
expose its derived current `parent_task_id`. Reparenting invalidates old and new
parents; it does not change the journaled opening request.
Existing user and agent reviewer assignment and durable agent dispatch remain.

Task detail carries the newest 20 `linked_reviews` with an explicit truncation
flag; child review changes invalidate its parent's detail for fresh status.
Ordinary work tasks render child review links only from their own cached detail
projection as “Review #N · <status>”,
gated by `tasks.review.forTask` so older bridges keep their ordinary task UI.
When there is no child review, linked workspaces offer “Request a review from
@workspace” leading to Changes. Task-side create/update snapshot forms and the
workspace snapshot dialog are removed. Legacy reads and the explicit Close
legacy review action remain as described above.

## Verdict loop (#972)

The author is an optional agent ID saved when the review opens: MCP uses the
calling agent; user openings use the workspace's bound agent. Publishing also
tracks that author. Legacy reviews resolve the current workspace agent for
verdict delivery.

The review task's assignee and the user are deciding reviewers. Only their
latest verdicts on the current revision determine status; other agents' opinions
are advisory. Changes requested takes precedence over Approved. A deciding
reviewer may Reject with a nonempty reason. Rejected is terminal and moves the
task to Done while retaining the workspace; it can reopen like Closed.
Approval is handed to the author, who chooses the next action and comments on
what happened. Agents can call `merge_review` only while Approved; the user's
RPC merge retains its existing behavior. Reassigning the reviewer recomputes
status. A changed status journals the new assignee’s existing verdict and original
batch once; unchanged status sends nothing. Verdict identity prevents repeat
delivery when a reviewer is assigned again.

Deciding verdicts atomically journal delivery with their comments. Each immutable
batch contains the reviewer's anchored comments since their last verdict on that
revision and ends with the verdict comment. Messages identify the task, revision,
verdict and every comment anchor (path, line, side and optional commit). Request
changes asks the author to fix each comment, commit and `push_review`, replying
where they disagree. Approval asks them to take the work forward and report the
action; rejection asks them to stop without merging and gives the reason.

A bridge service drains pending delivery without a browser. Pending messages
survive restart, stopped authors and usage limits. A missing author falls back
to the workspace's current agent; if none exists, a task comment mentions the
user because nobody received the verdict. Failed delivery is explicit and
retryable with `tasks.review.retryDelivery` / `retry_review_delivery`. Review
reads carry the latest 100 delivery summaries with state and comment anchors;
comment bodies remain in the timeline and durable journal. First acceptance is
ordered independently of mutable timestamps. Polling accepted pending deliveries
rotates without write churn. A superseded failed-verdict retry retains its batch
but reports current status as context only, without stale action instructions. New
revisions send an agent reviewer back to `since` from the previous revision or
return a user reviewer to Needs you. Superseded pending revision handoffs
settle without waking an agent reviewer; it receives the latest active revision
from its last accepted message baseline; waiting/cancelled assignment intents
never count. Blocked publication retains the task column. An unknown baseline
requests the full revision.
Transient dispatch failures allow seven attempts with 30/60/120/240/480/600-second
backoff delays and no writes on skipped polls. Permanent
or exhausted failures settle as Failed and post exactly one reserved-identity
`revision_undelivered` Build mention for the user.

Build reminders carry `review_notice: "revision_ready"` or
`"verdict_undelivered"` or `"revision_undelivered"`. Only those marked Build mentions enter Needs you;
ordinary Build-authored mentions retain their existing ignored behavior.

Wire 3.30.0 announces `tasks.review.author`, `tasks.review.reject` and
`tasks.review.verdictDelivery` independently. SPA controls and delivery surfaces
follow in #973.

## Published PR contracts (#405)

Wire **3.15.0** publishes the PR services landed in #401–#404. This section
describes the implemented PR mode; the snapshot-retirement amendment above governs
legacy reviews. The code and `ARCHITECTURE.md` remain authoritative.

A PR is an ordinary task with `mode: "pull_request"`, dedicated workspace
branches and registered local receiving repositories. Opening records a configured
or explicitly selected base for each included Git directory; non-Git directories remain Live and
unavailable Git directories must be excluded. Committed pushes to the receiver
publish immutable snapshots without a connected browser. Working files remain
outside the saved commit tree. Dirty state is counted, never staged by review.

| RPC | MCP | Inputs beyond task/review version |
| --- | --- | --- |
| `tasks.review.open` | `open_review` | Workspace ID, request ID, title, description, optional user/agent reviewer and `for_task_id`, per-directory bases and excluded Git directory IDs. |
| `tasks.review.push` | `push_review` | Selected directory IDs and expected working heads; an explicit force-with-lease names the expected received head. |
| `tasks.review.update` | `update_review_base` | Per-directory base branch selections. |
| `tasks.review.merge` | `merge_review` | Latest snapshot ID and all included directories' expected base heads, with optional external push destinations. |
| `tasks.review.close` | `close_review` | A short description of the action taken. |
| `tasks.review.reopen` | `reopen_review` | No additional selection. |
| `tasks.review.refresh` | `refresh_review` | No additional selection. |

`open` reserves one identity per project/request ID and returns the published
task/review, reviewer dispatch state and local push instructions. It can return
a published review with failed reviewer delivery. Its explicit retry reads or
recovers that same opening. The other mutations use the current review version;
stale requests refetch before an explicit new call. Nested selections accept
only their declared fields; callers cannot supply actor identities, source paths,
receiving repositories or command arguments. RPC and MCP share trusted identity
resolution, project scope, Git services and structured error mapping.

The `tasks.review.pullRequests` capability gates PR mode, summaries and observations;
each new RPC verb is also independently announced. Shared `get` and `diff`
continue to read both modes. Legacy `snapshot` and `act` mutations refuse as
described above; `complete` is reserved for legacy close-out, while PRs use
`close`. Git trees/blobs, anchored comments and PR opinions continue through the
existing reads and task comments.

Only each actor's latest opinion on the current published snapshot contributes:
any request for changes yields ChangesRequested, otherwise an approval yields
Approved. Open/Approved map to In review, ChangesRequested to In progress, and
Merged/Closed to Done. Ordinary movement to Done closes a PR. Reopen is available
only for Closed, unmerged reviews with the original managed workspace, dedicated
branches and receiving setup recoverable. It publishes a new snapshot even when
received heads are unchanged. Closing and merging do not finish or delete the
workspace; ordinary lifecycle and explicit removal remain separate. Explicit
refresh on a retained terminal PR records observations without reopening or
changing history.

Receiver retention (#839): merged PRs release their receiver data in background
maintenance. Closed, unmerged PRs keep it only while the original managed workspace
remains ready; removing that workspace allows cleanup and prevents reopen. Review
metadata and comments remain, while Git content can become unavailable after
reclamation. Running or unsettled merge publication retains its receiver until
saved retry work settles. Unused owned receivers and their operation journals are deleted.
Retained receivers receive startup GC with Git's normal pruning grace period,
because immediate pruning can race native pushes. New receivers hardlink immutable
source object files where supported, keeping independent entries that survive
source GC and removal; cross-filesystem receivers fetch reachable history instead.
Object alternates are rejected because native pushes cannot safely maintain source
protection refs through publication. GC at creation cannot eliminate live history.

Review reads preserve `{review}` and optionally include per-directory `sync`
observations with their own revisions and durable `merge_intents`. Push replies
also include per-directory outcomes. Task get/list and watched-task feed rows
optionally carry `review_summary`; workspace get/list optionally carry
`active_review`, retaining the latest terminal link when no active PR remains.
Existing task/workspace invalidations drive refetches. No new push event or
subscription kind is required.

Partial results retain successful repository facts. Only proof that every
reviewed head remains received and integrated into its configured base sets
Merged. Failed external publication after local integration leaves Merged with
the failed push visible; an explicit retry publishes the recorded merge tip and
does not rerun integration. A running merge or unsettled saved publication holds
workspace reclamation. Startup recovery observes recorded facts and never
automatically replays Git.

Both contract suites read the same fixtures, including legacy Snapshot examples,
PR task/workspace summaries, receiver failures, rewritten pushes, partial merges,
reviewer delivery failure and structured refusals. The previous-minor manifest
is generated from wire 3.14.0 on main before the bump. These contracts publish the
backend and agent entry points; subsequent SPA work consumes the cached flags
and saved results.

## Saved revisions and review reading

A review records committed work in every included Git source directory, with a
saved branch, comparison base and head per revision. Changes shows the committed
diff; Files browses the whole saved head tree, including unchanged files.
Uncommitted files are counted and excluded. Non-Git directories remain explicit
Live context and use their exact saved directory identity. Missing repositories
show Source unavailable rather than silently substituting live Git content.

Metadata, comments and action history live in SQLite; immutable Git content stays
in the retained receiving repositories for PRs and recorded repositories for
legacy reviews. Closing a review retains history. Receiver reclamation follows
the retention rules above; legacy refs remain until explicit history deletion.

The review page keeps the task timeline beside a revision picker, directory
switcher, commit rail and Changes/Files views. Revision and commit selections
live in the URL; viewed-file marks and comment drafts remain personal durable UI
state. New revisions leave the reading place and older drafts pinned. A verdict
is explicit and applies only to the latest published PR revision. Approve and
Request changes submit pending anchored comments before the deciding verdict;
Reject requires a reason and ends the review without merging.

All rendering paints from cache. Reads and pushes populate project-owned review
records and snapshot/directory/path body caches before redraw; capability flags
are cached independently. A late response may populate only its own saved
identity. Live workspace Notes to agent remain separate from immutable revision
comments and do not imply a review verdict.

## Historical design

The original snapshot-mode rollout and selected Merge/Push UI are preserved in
this document's Git history. They are retired product behavior. The workspace
opening, verdict loop and snapshot-retirement sections above describe the
current review flow.

Cold workspace ownership: workspace get/list and feed rows expose optional
`legacy_review: true` under `tasks.review.snapshotRetired` in wire 3.31.0. This
comes from persisted review workspace ownership, including completed reviews
whose tasks have no workspace link, so direct branch visits hide legacy
Merge/Abandon controls before task or review metadata has been cached.
