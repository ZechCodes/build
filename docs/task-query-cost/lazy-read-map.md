# Lazy view reads: ownership and replacement reads (#894)

This is a source audit of the final cutover implementation. Actual matched
compatibility/performance and targeted gate receipts are linked below; the
coordinator's full SPA acceptance gate remains separate. The supported-version
floor was not approved for retiring legacy paths, so those paths remain.

The matched source cohort at `50d4a2647` now records five successful fixture
processes, exact-base/candidate source verification and mandatory copy-based
restoration in [the final cost manifest](cutover-manifest.json). These are
synthetic-peer/source compatibility and cache/mount measurements, not installed
bridge deployments or the coordinator's full SPA acceptance gate.

The final re-review runtime is `907ae03e33`, with [completed positive and
control receipts](review-round-2.md) and [fresh source-pinned costs](cutover-review-907ae03e33/manifest.json).
The original 50d records remain separately attributed.

Review boundaries, in dependency order:

- `d03d2a747`: stop hidden warming under scoped ownership.
- `2113fe95b`: selected task detail/history and shared scoped continuations.
- `457f9be5e`: directory pages and root-free file invalidation hints, wire 3.35.
- `6d38b4fb1`: coalesced transaction publication accounting and append budget.

## Ownership and fallback

`scopedSyncRuntime.lazyViewReadsEnabled(deviceId)` requires
`deviceScopedSyncOwns(deviceId)` plus current `sync.projectTasks` and `sync.task`
capabilities. Device ownership also requires the browser lock API, a tracked
device scope, current `sync.device`, and wire >=3.34 through
`deviceSyncSupport.supportsDeviceSync`. A cached capability or continuation does
not authorize a request after a new greeting. Without this ownership, legacy
inbox/background warming and legacy push subscriptions remain in place.

Stopping warming and selecting paged request shapes are separate decisions.
`tasks.timelinePaged` and `fs.treePaged` are negotiated wire-3.35 capabilities;
3.34 ownership alone does not authorize either opt-in. `taskTimelinePages`
selects `{task_id}` on a legacy greeting; `directoryPages.directoryPageParams`
returns `{}` there. A cached newer cursor never rides a legacy request. Old SPAs
continue to use existing unopted bridge contracts.

The 3.34 scoped task head predates shared continuations. On selected-task open,
`scopedTaskBody.refreshScopedTaskBody` uses the already-required legacy
`tasks.get` body read to retain the full timeline, marks
`timeline_legacy_complete`, and preserves that connected history across later
partial heads. The body basis includes protocol mode, so a same-revision
3.35 -> 3.34 rollback cannot skip this supplement. An Older action holding a
newer cursor falls back to a selected full legacy read under the current
greeting. Returning to 3.35 restores negotiated bounded reads.

## Removed warming -> open-time owner

Paths below are relative to `spa/src/core/` unless marked otherwise. The gate
suppresses work only under the ownership predicate above.

| Former hidden path | Scoped behavior and replacement read |
| --- | --- |
| `inbox.cacheableEntityIds`: Inbox/Recent partition plus an active branch's implementation task fed the warm entity set | Function and legacy meaning remain. `cacheSync.workspacesToRead` returns no warming order under lazy ownership. Mounted `agentRail.watchConversation`, `mountGitPane`, and Files readers select actual demand instead; an Inbox row or routed checkout does not open all its tabs. |
| `syncPhases.foregroundJobs` / `routedJobs`: routed tree, checkout and task work | `lazyForegroundJobs` selects only `visibleSyncThread` and `visibleSyncReads`. Registration through `syncVisibleScopes.registerSyncRead` wakes the pass; changing the route alone does not warm checkout bodies. |
| `syncPhases.readInboxThreads`, `readInbox`, `readBackgroundEntity`, `readBackground`, and roster/background stages in `createSyncPhases.run` | Lazy mode skips these stages. Rechecks after foreground yields and before `takeJob` prevent queued legacy work from starting after ownership changes. Open chat/tree/Git/diff registrations provide the replacement reads. |
| `cacheSync.syncStatus`, `listTrees`, `syncLog`, `syncUnpushed`, `syncWorkingDiff` through checkout phases | No background checkout walk. `gitPane.standUp`, `forceRefresh`, `syncReaders`, Files' `mountFileTree.load`, and selected-file `pullFile` read their mounted surfaces. Git status uses its held key; unpushed sync reads metadata with `patch:false`. |
| `cacheSync.syncWorkspaceExtras` and `rereadHeldFiles`: recovery of recently cached bodies and working diffs | Lazy early returns prevent cached-but-closed files from being reread. Files' selected-file reader and expanded diff bodies revalidate on open; mounted visible reads provide missed-event recovery. |
| `cacheSync.syncPatches` / `warmPatch` / `PATCH_WARMING_SKIP_KIND` (`patch-over-cap`) | No speculative unpushed commit patch requests or skip-marker maintenance under lazy ownership. `gitPane.fetchShow` / `readShow` fetch the selected commit, and `readCommitPage` continues its patch. An old over-cap warming marker never blocks an open commit read. Markers and warmers remain for legacy bridges. |
| `fileDiffs.createFileDiffs().warm(status)` | Returns zero without filling collapsed file bodies. Git Changes uses `sync({status,openPaths})` for expanded files, then `more(path)` when that body needs its next page. |
| `cacheSync.subscribeDevice` all-scope thread/git/files subscriptions and `followRoutedEntity` checkout body subscriptions | Lazy subscriptions retain summary/runtime demand; body subscriptions do not follow every Inbox row or route. `followVisibleThread` binds one mounted conversation. `gitPane.bindCheckoutWatcher` and `directoryWatcher.createDirectoryWatcher` bind mounted checkout readers and rebind after greeting/ownership changes. |
| `cacheSync.applyThreadItem` / `deferThreadCatchup` background repairs from pushes | Transcript work is limited to a currently visible conversation, including a lifetime recheck during queued catchup. `agentRail.watchConversation` seeds cached content and always calls `threadSync.syncThreadWindow` in lazy mode, even when the seed succeeded. |
| `cacheSync.applyGit` / `pullWhatTheGitItemCouldNotCarry` and missing diff pulls | Lazy mode calls `refreshOpenReads` only for registered status/log/unpushed/diff readers. A hidden checkout does not pull patches from an invalidation. `applyLegacyGit` retains the old producer-payload behavior. |
| `cacheSync.applyFiles` / `rereadHeldFiles` | Lazy `directoryPages.applyDirectoryHint` marks affected held tree records stale without discarding their entries; only visible tree and matching selected-file readers refresh. Folded/closed directories wait for open. `applyLegacyFiles` and legacy body recovery remain. |
| `trackerTaskDetailsFeed.fetchDetail` / `queueIfBehind`: list or Dashboard summary changes warmed task bodies and complete timelines | Lazy mode leaves the cache observer active but stops unselected detail fanout. `trackerTaskPage.requestTaskRead` / `readTask` owns selected demand: register/wake the task scope, then the revision-fenced `scopedTaskBody.refreshScopedTaskBody` supplements its body; a non-scoped owner uses `requestTaskDetail` directly. |
| `cacheSync.readReviews` / `readWorkspaceReviews` / `readGitReviews`; `taskReviewCache.refreshCachedReviews` discovers cached or hinted task IDs | Lazy mode skips the fanout. Task review surfaces use `createTaskReviewRepository.refresh` for their selected task. `workspaceReviewReads.createWorkspaceReviewReads` opens just the visible Changes header's selected review and selected task, described below. |
| `cacheSync.applyTasks` legacy list/review refresh after an event | Scoped sync is woken first, then lazy mode stops the legacy list/detail/review fanout. Project/task scopes publish the summary/header state; opening a detail or review supplies its bounded body demand. |

`taskReviewCache.refreshWorkspaceReview` is retained as a selected-workspace
metadata reader, not a sweep of every cached review. `readWorkspaceReview` is
cache-only. The new `workspaceReviewReads` owner resolves one task from the
visible review or matching workspace hint, registers that task scope, and
refreshes its `tasks.review.get` metadata. With a paged scoped owner it wakes
that task scope; otherwise it performs one greeting-fenced selected
`tasks.get`. This supplies authoritative `last_reviewed_snapshot_id` even if
the Task page was never opened. `workspaceReviewEntry.setVisible` and
`views/workspaceChanges` suspend this owner and release its scope on hide,
reacquiring selected demand on show. Selection/version/greeting changes may
refresh this one owner; there is no `refreshCachedReviews` fanout.

## Open reads and their bounds

| Surface / actual function | Bound and continuation |
| --- | --- |
| Conversation: `agentRail.watchConversation` -> `threadSync.syncThreadWindow` | Cold lean page requests 23 messages plus lean activity projection; legacy cold and warm forward requests use `LATEST_THREAD_ITEMS=100`. Recent in-place repair uses `REPAIRED_THREAD_ITEMS=50`. `agentRail.fetchOlderPage` -> `agentRailContext.olderThreadPage` uses `before_sequence`, with the bridge's bounded default of 20. Older demand has one read in flight, widens the shared cache, and is driven by scrolling/pressing for earlier entries. |
| Task: `taskTimelinePages.requestTaskDetail` / `taskDetailReadParams`, scoped `task_head` | Paged selected reads request 50 rows (API accepts 1..100), newest window in ascending order. Each row <=64 KiB, window <=256 KiB, full response <=1 MiB; imported task records >512 KiB are refused before body decode. Ordinary body writes already enforce 32,000 bytes; the canonical body is one field, not history paging. `before` is the opaque older cursor. See [bounded task reads](../bounded-task-reads.md#bounded-task-detail-and-history-wire-335). |
| Task Older / old comment and reply: `trackerTaskPage.readOlderHistory`, `readHistoryPage`, `readRoutedComment`; `taskTimelinePages.prependTaskTimeline` / `keepTaskTimelineAnchor` | Older prepends one cursor-owned window. A routed comment or unloaded reply uses `timeline_anchor` to seek one bounded window, shown separately with a gap boundary rather than draining intervening pages. At most four anchor pages are shared in cache; each mounted view retains its own selected target. Server counters and `timeline_read_through` preserve all-history unread/attention semantics; `last_reviewed_snapshot_id` preserves the last opinion outside the window. Older author facts merge without overwriting current identities/body/watch facts. |
| Dependencies: `mountTaskWaitingOn`, `createTaskWaitingOnActions.blocks`, `taskWaitingOnCache` | Selected task detail supplies at most 20 forward dependencies (write contract) and the first 20 reverse `blocks`, plus counts and `blocks_next_cursor`. Only the More control calls `tasks.waiting_on.blocks` with the cursor (default 20, API 1..100). `readChoices` reads the cached project task selector; it does not fetch every candidate's body or timeline. Counts on summary rows do not trigger graph expansion. |
| Files / live review Files: `mountFileTree.load`, `readMore`; `directoryPages.directoryPageParams`; `taskReviewFiles.listDirectory` | Root and expanded directories request `fs.tree {paged:true,limit:100}` when negotiated. More requests that directory's opaque cursor. Refresh atomically rebuilds the held window toward its old tail sort key, bounded by `loaded_pages` (or `ceil(entries.length/100)` for an older cache) and at most 8 pages per directory; reveal has at most 8 extra continuation reads shared across ancestors and stops once found. Truncated hints refresh every expanded visible directory; folded stale caches stay cold. Cached state preserves the open row/reading place within the bounded window. Uncapable bridges receive the old parameter shape. Snapshot review tree reads retain their existing `tasks.review.diff` contract. |
| Selected file: `views/files.pullFile`, `pageReader`, `storePulledFile`; `cacheLifetime.filePageReader` / `readFilePages`; `taskReviewFiles.readFile` | First `fs.read` uses the existing 1 MiB wire cap (raw selected media explicitly requests 1 MiB). Source continuation uses `BODY_PAGE_BYTES=262144`. Media/rendered files deliberately fetch the selected whole body in <=1 MiB chunks, capped at `FILE_MEDIA_MAX_BYTES=64 MiB`, because display needs a complete body; this is open-file demand, not directory warming. Unknown binary bodies remain facts-only. Body-page version changes stop the old chain. |
| Conversation attachments: `threadAttachmentPages.readThreadAttachment` | The selected attachment is assembled in 1 MiB pages when supported; legacy reads retain their old shape. Attachment loading is an explicit open/download/lightbox demand, not transcript background warming. |
| Task attachments: `taskAttachmentBodies.createTaskAttachmentBodies().load` | Selected `tasks.attachment` reads assemble 1 MiB wire chunks at background priority; cached attachment bodies paint without rereading. No list or timeline warmer requests these bytes. |
| Changes: `gitPane.syncReaders`, `createFileDiffs.sync`, `readPage` | Working changes can read shape separately; only expanded paths request bodies, batched at <=50 paths. One file body caches up to 1 MiB as a whole record, otherwise versioned pages; subsequent `git.diff` ranges use 256 KiB. Collapsed files stay cold. |
| Commit list: `gitPane.readCheckoutRecord('log')`, `showMore` | Open list uses bounded server browsing default 30 (API clamps 1..200); older rows use `skip` on reader demand. Legacy sync's cursor refresh budget is 20 (`LATEST_COMMITS`), a different request from browsing. Reopening reuses held pages; no speculative commit-patch sweep follows list rows. |
| Selected commit patch: `gitPane.fetchShow` / `readShow`, `readCommitPage`, `readMoreOf` | First `git.show` has the existing 1 MiB wire patch cap; the 256 KiB whole-record cache threshold determines whether it is stored as a head with pages. Continuations use 256 KiB ranges when `bodies.pages` is current. `commitDiffTarget` may seek up to 200 pages for an explicitly targeted line; ordinary opening does not drain history. Legacy bridges retain their capped answer without unsupported `range` params. |

## Cache paint and silent fences

A cached mount paints its stored record first: `agentRail.watchConversation`
seeds the conversation, `trackerTaskPage.paintFromCache` runs before
`refreshForMount`, `gitPane.standUp` paints `readRecords`, `fileTree.load` takes
held listings before asking, and selected Files uses its held record before
`pullFile`. Reads write through the existing cache repositories and cache
announcements drive the visible redraw. Pending refresh does not replace held
content with a loading screen. Cacheless low-level fixture mounts have no
repository and retain their direct-answer behavior.

Scoped task supplements require the captured task revision, body basis,
mutation/content read authority, and current greeting. Rejected body writes
retain the held timeline and its entire paging metadata family together;
an unaccepted cursor cannot attach to a newer timeline. Older/anchor merges
also check the current cursor and timeline incarnation, and per-mounted seek
serials stop reversed replies from moving another view's selection.

Task cursor fences are typed `invalid_params` with
`details.reason='task_timeline_cursor_invalid'`. An Older action silently
refreshes the selected first window/scope while keeping cached content.
`fileTree.recoverRefusedCursor` likewise restarts a refused selected directory
continuation without presenting a sync error. These fence recoveries have no
loading, toast or error UX. Genuine history/read failures retain ordinary
error handling. Scoped sync recovery itself supplies no user-facing fence
state.

## Visible owners, hidden body retirement and root reuse

Scoped Git watchers are owned by the shown pane and document. `gitPane`
`checkoutReadsAllowed` stops metadata/body/selected-patch work while hidden;
`bindCheckoutWatcher` disposes the old binding and advances its generation on
hide or re-greeting. Showing the pane/document paints held records and
revalidates only its selected status/log/unpushed/Changes readers. A frame or
queued cache write from the old binding cannot become the new owner's data.
The unscoped 3.33 watcher path retains its existing hide/show behavior.

Files distinguishes modern hints from legacy body-bearing subscriptions.
`directoryWatcher.createDirectoryWatcher` may retain root-free `files.hints`
while folded, marking held listings stale without fetching hidden content.
With scoped 3.34's legacy `files` payload, folding the root or hiding the
document retires that body subscription. Returning reacquires the watcher
and refreshes visible demand from cached content. Its generation predicate
fences a frame that was awaiting cache admission when the owner changed.

Live task-review Files reuses the exported `directoryWatchSpec` selector for
exact `project_source` + `files.hints` ownership. If that subject is unavailable,
its modern fallback registration serves explicit/reconnect refresh only;
ordinary board state causes no filesystem reads. Matching hints read only
visible touched directories and selected-file demand. Generation predicates
reach `fileTree.invalidate` and the selected body's existing cache `still`
guard; retired or currently hidden readers reject late hint answers. The legacy
workspace/files watcher branch retains its existing behavior. Continuation
`invalid_params` is not persisted as a review error: generic recovery keeps
cached rows/preview painted while obtaining a new head. Genuine first-page
and unavailable-continuation errors remain visible.

`directoryPages.legacyRootForVisibleReader` also preserves the useful visible
legacy contract: a valid complete root listing already carried by a 3.34
`files` frame is published first when that root is on screen. Atomic admission
checks the current greeting/binding, root visibility and captured record write.
An accepted root is marked fulfilled, so only other affected expanded
directories are requested; no duplicate root RPC is made. Folded/hidden roots,
foreign or malformed bodies, newer cache writes and modern hint-capable
greetings cannot admit that legacy body. The modern hint remains root-free.

Source-owned cache guards reach the transaction itself. The optional mounted
owner passed to `cacheSync.applyProjectSourceChanges` controls queue admission,
each source applier and atomic record writes; retirement before or during
IndexedDB admission rejects the old frame. `writeDirectoryGit` commits a
status/unpushed update and its dependent invalidations together, rechecking
the active owner and captured record write inside the transaction. A stale
answer cannot mark a newer log/diff stale after losing its own admission.
These guards preserve the legacy unowned producer path and do not expose
sync fences to the renderer.

Exact regressions are in `checkoutWatcherLifecycle.test.js`,
`directoryLegacyRoot.test.js` and the owner-admission/transaction cases in
`cacheSyncChanges.test.js`; source-specific visible read fencing remains in
`syncVisibleReads.test.js`. The secondary-workspace-source board/state
fallback still lacks an independent live filesystem subject, as stated in
[the coverage boundary](cutover-coverage.md#filesystemsource-view-boundary-and-unsupported-claims).
Open/reconnect validation does not imply a new filesystem event producer.

The absent-birth-time provider uses the bounded held-handle fallback in
[the directory contract](../fs-tree-pages.md): 32 retained leases, absolute
120-second expiry, refusal on eviction/expiry/replacement, and no relaxation
of missing Unix device/inode identity. Real Rust provider tests and separately
scripted SPA DTO recovery tests compose this boundary; no installed bridge
or browser-to-Rust integration run is implied.

Subsequent review corrections and their separate qualification status are
tracked in [review round 2](review-round-2.md). The original matched cohort
continues to describe its recorded `50d4a2647` source; those observations were
not rerun merely because these contract fixes landed.

## Read coordination and priority audit

`readRequests.coordinatedRead` is a scheduler/deduplicator, not a source of
warmer demand. Its key includes device, request scope, repository, transport,
method and params. Foreground starts immediately; queued background waits for
active foreground readers and an idle turn (2,000 ms idle deadline), then
rechecks. A matching foreground caller promotes queued work. Stopped reader
lifetimes no longer hold foreground quiet, and a pending request without active
readers is replaceable. Settled requests are forgotten.

`requestPriorityFields` stamps only `{priority:'background'}`. In
`sessionRpc.call`, priority defaults to foreground and is carried beside
method/id/params only for background. Background SQL admission has a separate
bounded credit wait; foreground remains the absent envelope field understood
by old bridges. These primitives remain for legacy warming and legitimate
selected media reads; deleting them would change unrelated/legacy behavior.

Open conversation requests explicitly use foreground. Task and selected-review
reads use ordinary foreground dispatch. File diff coordination defaults to
foreground; missing/expanded bodies are not silently made background warmers.
Files intentionally sends selected raw-media reads and later media pages with
background priority so a large selected file yields to interactive work.
Attachment downloads also use background priority. This priority changes
scheduling only: selection/lifetime predicates still own the
read, and it does not authorize hidden files or unopened histories.
