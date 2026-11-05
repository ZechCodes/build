# HTTP consumer coverage after the #894 cutover

Reviewed against merged source `08e22bac014bf20e56c70fb9a3711e8ef304a085`
and dev `3e6e81bd960ec8cddc993c8b0ff9fa74b856bdcc`. The initial coverage run
exited 1 with exactly subtests 12, 14, 16 and 17 failing; 13 of 17 passed.
This inventory repair changes no SPA/bridge behavior, wire version, HTTP route,
exclusion, scanner rule, dynamic-site inventory or assertion strength.

## Finite metadata occurrences

The adapter's `capabilitiesOf` return object now reads three additional named
capabilities: `changes.filesHints`, `fs.treePaged`, and `tasks.timelinePaged`.
They are Boolean negotiation metadata, not methods. Existing capability labels
remain in the same return statement; its token fingerprint therefore changes
for all those finite occurrences. The prior statement fingerprint was
independently reproduced before calculating the new one. Context, occurrence
count and full statement fingerprint remain pinned for each exemption.

Seven further finite exemptions identify the `files.hints` push-kind label:
one in `changeEvents.forKinds`, one in `directoryPages.chooseFilePushKind`,
two in `directoryWatcher` (subscription retention and pause policy), and three
in `taskReviewFiles` (modern refresh, legacy dispatch choice, and watcher
selection). None is passed as an RPC method. Reusing a label as a method or
adding another occurrence still triggers the existing coverage guards.

## Reviewed dispatch and source pins

| File under `spa/src/` | Reviewed change and concrete method ownership |
| --- | --- |
| `core/agentRail.js` | A cached seed no longer prevents validation when lazy sync owns the view. Existing context RPC dispatch is unchanged. |
| `core/bridgeApi/v1/index.js` | Adds the three Boolean capabilities above. Adapter transport remains `call(...args)`; no new verb. |
| `core/cacheSync.js` | Scoped ownership removes background conversation/checkout/detail warming, narrows push subscriptions, and routes visible Git/file demand through existing readers. Legacy `ask` / `callWhileActive` / context dispatch sites and their methods remain unchanged. Source-owner predicates additionally fence cache admission. |
| `core/changeEvents.js` | Adds disabled capability defaults and routes a carried `files` payload to a `files.hints` watcher. The new string is a push kind; transport dispatch is unchanged. |
| `core/directoryPages.js` (new) | Chooses push kind and negotiates paging parameters; marks held cache records stale and invokes registered visible readers. No RPC verb is produced. Its push-kind occurrence makes this a reviewed metadata source. |
| `core/directoryWatcher.js` (new) | Selects entity/project-source/board subscriptions and visibility ownership. Its two push-kind comparisons are metadata; it introduces no RPC verb. |
| `core/fileDiffs.js` | Suppresses `warm` under lazy ownership. Existing body/diff reads retain their verb selection and paging. |
| `core/gitPane.js` | Adds visibility/greeting lifetime checks and selected push kind. `forceRefresh` gains only `active = alwaysActive`; default kinds and the dynamic template expansion remain exactly `status` and `log`, producing `git.status` and `git.log`. Action dispatch remains inventoried unchanged. |
| `core/taskReviewCache.js` | Stops background `refreshCachedReviews` under lazy ownership. Existing finite `tasks.review.${verb}` producer and mutation adapter remain unchanged. |
| `core/taskReviewFiles.js` | Negotiates `fs.tree` paging for live plain directories; retains `fs.read` and saved `tasks.review.diff`. Modern live workspace registration handles refresh/reconnect only. Body/tree cache writes gain ownership guards and cursor fences remain quiet. No new method. |
| `core/taskTimelinePages.js` (new) | Owns the explicit `options.callRpc("tasks.get", params)` read. The greeting selects legacy full replies or bounded first/older/anchor parameters; it never changes the verb. Page/cache merges retain generation and cursor ownership. |
| `core/trackerTaskPage.js` | Direct detail reads delegate to `requestTaskDetail`; older/anchor demand uses the same `tasks.get` owner. Existing action, watch and read-through dynamic dispatch remains unchanged. |
| `core/workspaceReviewEntry.js` | Visibility-owned `createWorkspaceReviewReads` replaces reliance on background warming. Existing review branch/action RPC wrappers remain unchanged. The helper delegates metadata to the existing review repository and detail to `requestTaskDetail`; it produces no additional verb. |
| `views/files.js` | Existing `fs.tree` receives capability-gated paging parameters; `fs.read` gains selected-file/visibility ownership. Existing upload/file-menu and secret-preview behavior from dev remains intact; no new method. |

Two former direct-dispatch pins transfer to the new task detail owner:
`core/scopedTaskBody.js` and `core/trackerTaskDetailsFeed.js` now call
`requestTaskDetail`, passing their captured `callRpc` and lifetime predicate.
They no longer contain a recognized method literal/call/producer, so the
unchanged scanner no longer includes them in its source map. Their former
`tasks.get` dispatch is explicitly pinned in `core/taskTimelinePages.js`.
No namespace exemption or general indirect-dispatch exemption was added.

All 14 retained/new source entries use exact full-file SHA-256 fingerprints,
displayed as eight short parts. Eleven existing entries change, three enter,
and the two delegated direct-dispatch entries leave. Unchanged dev source
fingerprints, including the secret/file-menu additions, remain untouched.

The resource map needs no additions: task timeline first/older/anchor reads
use existing `tasks.get`; directory pages use existing `fs.tree`. The three
capability names and `files.hints` push kind are not HTTP operations. The
17-test guard run is still owned by the coordinator; its actual post-repair
receipt will determine the result.

## Logical conversation opening admission follow-up

The rail now admits lazy revalidation once per logical opening, using the
existing thread address and incarnation, selection generation, and actual
device session. The bounded per-mount admission registry survives a transient
reader being replaced by canonical history. A display-only held window keeps
cached words painted during that same-address handoff until canonical cache
seeding completes; it never overwrites canonical history. Real reselection,
reopening, session replacement and thread-incarnation changes remain new reads.
The legacy seeded-cache short circuit stays in its non-lazy branch.

Reviewed the complete agentRail change and its outgoing call assignments:
readConversationWindow extracts the same syncThreadWindow request fields and
foreground priority. No RPC verb, parameter shape, resource mapping,
capability exemption, dynamic producer or ownership edge is added. Only the
rail's full-source fingerprint changes; the lexical scanner, metadata
occurrences, method inventory, helper recognizers and all assertions are
unchanged. Stress and lifetime tests retain the one-read expectation.
