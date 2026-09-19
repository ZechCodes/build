# Primitives: local-feel diffs and chat (bridge-git, bridge-thread, spa-diffs, spa-chat)

> **Amendment (2026-09-18, Zech):** `post_thread_message` takes only `status`,
> `body` and `options`. There is no `phase`, `outputs`, `anchor` or `links`:
> Build knows which phase a report closes from the session that sent it, and a
> plan's stages are read from `.build/plan/stages.json` on disk when the plan
> agent reports Complete. The per-stage validation gate (validate/fix-stage
> sessions, `ValidationReport`, `run.stage_fix`/`issue.stage_fix`), diff triage
> (`triage.override`, `triage_enabled`, `.build/review-rules.json`), the
> branch-recovery agent (`RecoveryAttempt`, `phase=recover`) and agent-reported
> comment resolutions are removed. A stage is `building` until its build
> reports Complete, then `completed`. Where this document says otherwise, this
> note wins.

Four items, one idea: **the wire carries shape; bodies are fetched per unit and cached; a surface renders only what is open.** Build the primitives below, then compose. Names here are binding — a Fable checkin holds both halves of every wire shape to this doc.

## Bridge primitives

**1. The status shape and its key** — `bridge/src/gitgui.rs`
- `fn status_shape(repo_path: &Path, max_files: usize) -> Result<(Value, String), String>` — the libgit2 walk only: branch/head/repo_state/upstream/ahead/behind/stash_count/files/files_truncated, plus each file's `content_key`, and the hash of all of it.
- `struct ContentKeys { worktree_root: PathBuf }`; `fn key_for(&self, path: &str, entry: &git2::StatusEntry<'_>) -> String` — the **working-tree** content, never the index: `entry.index_to_workdir().new_file().id()` when that oid is non-zero (libgit2's workdir oid), else `fnv1a64_hex` over `size:mtime_nanos` of the file under `worktree_root`, else `"deleted"`. The index blob id is not usable here: for a tracked file with `worktree_status` `M` and an unchanged index entry it holds the staged/HEAD content and does not move when the working tree is edited again, so two successive edits would share one `content_key` and one `status_key`, and the SPA would serve a stale cached `filediff` body as current.
- `fn status_key(shape: &Value) -> String` — `fnv1a64_hex` over branch, head, repo_state, upstream, ahead, behind, stash_count, files_truncated and every file's `path|staged|index_status|worktree_status|content_key`. Line counts are **not** in the key.
- `pub fn status_payload_unless(repo_path: &Path, if_status_key: Option<&str>) -> Result<Value, String>` — the one entry point; on a key hit it returns the unchanged answer and never touches `crate::diff`. `pub fn status_payload` stays (stage/unstage/commit keep calling it).
- Hides: git2 status bits, the hash function, whether counts were computed.
- Reuse: make `diff.rs::fnv1a64_hex` `pub(crate)` — one hash in the crate.

**2. The per-file diff reader** — `bridge/src/diff.rs` + `gitgui.rs`
- `pub struct FileDelta { path: String, added: usize, deleted: usize, binary: bool }` and `pub fn uncommitted_file_deltas(repo_path: &Path) -> Result<Vec<FileDelta>, DiffError>` — one walk, no patch rendered; `LARGE_FILE_BYTES` / `added_lines` rules unchanged. `status`'s `stat` totals are the sum of these (one census, not two).
- `pub struct FilePatch { path: String, patch: String }` and `pub fn patch_for_paths(repo_path: &Path, paths: &[String]) -> Result<Vec<FilePatch>, DiffError>` — pathspec-restricted `diff_tree_to_workdir_with_index`, untracked file as all-additions, deleted as all-deletions, `is_mcp_config` excluded.
- `gitgui.rs`: `pub const GIT_DIFF_MAX_PATHS: usize = 50;` and `pub fn file_patches(repo_path: &Path, paths: &[String]) -> Result<Value, String>` — caps each patch at `GIT_SHOW_MAX_PATCH_BYTES` with `truncated`, attaches each path's `content_key`, answers in request order. Validation is the two predicates `stageable_paths` is built from — `crate::plan::is_worktree_contained_path` and `crate::diff::is_mcp_config` — but `git.diff` **errors on either**, it does not call `stageable_paths` itself: that helper silently drops an MCP config path, which would leave a requested path with no answer and make "answers in request order" ambiguous. A fence escape errors (`path escapes the worktree: <path>`) and an MCP config path errors (`path is not readable through git.diff: <path>`); both are the validation-failure test's target, and neither path ever appears in a status shape, so the SPA never asks for one.
- `app.rs`: `fn git_diff(&mut self, params)` through `defer_git(params, false, …)`, dispatched at the `"git.status"` arm (~6576).

**3. The settle window** — `bridge/src/changes.rs`
- `pub const ENTITY_SETTLE_WINDOW: Duration = Duration::from_secs(1);`
- `pub fn note_entity_settled(&self, id: &str)` — notes `Board` as today and `Entity(id)` under the window. `Pending` gains `settled: BTreeSet<ChangeKey>`; the bus holds `emitted_at: Mutex<HashMap<ChangeKey, Instant>>`. `flush()` leaves a settled key pending when its last emission is younger than the window, and calls `wake.notify_one()` when it held anything back so `run`'s next turn emits it. The same pass prunes `emitted_at` of every entry older than `ENTITY_SETTLE_WINDOW` — a key that old can never hold anything back — so the window adds no state that grows with the number of entities the bridge has ever seen.
- `app.rs::store_run_stat` (~4730) calls `self.changes.note_entity_settled(&run_id)` in place of `note_entity_changed`. Every other caller is unchanged and keeps the 250 ms window.
- Hides: the clock, the held-back set, the re-wake, the prune.

**4. The activity span reader** — `bridge/src/thread.rs`, `store.rs`, `app.rs`
- `pub fn run_items_shipped(run_len: usize, activity_left: usize) -> usize` in `thread.rs` — `run_len.min(PAGE_ACTIVITY_RUN_CAP).min(activity_left)`. `fold_activity_run` calls it; so does the store's assembler. One rule, one place.
- `Thread::activity_between(&self, from: u64, through: u64, before: Option<u64>, limit: usize) -> Vec<&ThreadItem>` (memory) and `Store::thread_activity_range(&self, agent_id, from, through, before, limit) -> Result<Vec<ThreadItem>, StoreError>` (SQL, `ORDER BY sequence DESC LIMIT`, handed back oldest-first).
- `thread_activity_range`'s predicate is **exclusive on both ends** (`sequence > ?2 AND sequence < ?3`) because that is what `conversation_span`'s gap read needs. `thread.activity`'s span is inclusive `[from, through]` and `before_sequence` is a third bound, so `AppState::activity_span` passes `from - 1` and `min(through + 1, before)` — one statement serves both the gap read and the verb, and the bound arithmetic lives at that single call site.
- `AppState::activity_span(&self, thread, from, through, before, limit) -> Result<Value, String>` — the single construction point that picks memory (`thread.resident_from_sequence() <= from`) or the store, and shapes the page. `fn thread_activity(&self, params)` is its handler, dispatched beside `"thread.page"` (~6566), reading `conversation_owner_param` + `named_agent_id` exactly as `thread_page` does.
- Doc comment states the immutability contract: a run older than the newest message never changes, so a client may cache it by `from_sequence` until the entity is evicted; only the tail run is live and arrives as forward deltas.

**5. The bounded run read** — `bridge/src/store.rs`
- Schema v5: `activity INTEGER NOT NULL DEFAULT 0` on `thread_items`, `CREATE INDEX thread_items_activity ON thread_items(agent_id, sequence) WHERE activity = 1`, `fn add_activity_column`, `classify_stored_items` extended with `item.is_activity()`, `SCHEMA_VERSION = 5`. `message = 1 OR attention = 1` is **not** a stand-in: lifecycle events are neither.
- New statements beside the existing consts (so the query-plan tests pin the statements that run): `THREAD_CONVERSATION_STRUCTURE_SQL` (`… AND activity = 0 AND sequence < ?2 AND sequence >= ?3 ORDER BY sequence DESC`), `THREAD_ACTIVITY_RANGE_SQL` (`… AND activity = 1 AND sequence > ?2 AND sequence < ?3 ORDER BY sequence DESC LIMIT ?4`), `THREAD_RUN_OLDEST_SQL` (same predicate, `ORDER BY sequence ASC LIMIT 1`).
- `THREAD_ACTIVITY_RANGE_SQL` is one statement with two callers: `conversation_span`'s gap read **is** `thread_activity_range` (not a second statement that happens to look like it), and `AppState::activity_span` is the other, with the bound arithmetic named in primitive 4.
- `conversation_span` becomes: floor seek (unchanged) → structure query → for each gap between consecutive structure items, fetch the newest `run_items_shipped(usize::MAX, budget_left)` activity rows **plus** the gap's oldest sequence (deduped). `cut_activity_runs` then yields byte-identical items and digests: the oldest row is what makes the digest's `from_sequence` right, and the cap drops it again. `THREAD_CONVERSATION_PAGE_SQL` retires.
- Census unchanged: `tool_calls_between` stays the SQL count.

## SPA primitives

**A. The cached-body fetcher** — `spa/src/core/cachedBodies.js` (new)
`createCachedBodies({ addressOf, fetchMissing, valueOf })` → `{ read(key), ensure(keys), has(key) }`. Decide → one async fetch boundary → write-through `writeCached` → answer. Session-hot Map in front, `readCached` behind it. Two configurations only: file diffs and activity runs.

**B. Per-file diffs** — `spa/src/core/fileDiffs.js` (new)
`export const FILE_DIFF_RECORD_KIND = "filediff"; export const GIT_DIFF_MAX_PATHS = 50;`
Pure decisions: `pathsToFetch(status, { openPaths, cached, triaged })` (open files whose `content_key` changed or is missing → eager; collapsed → only on expand or warm; `triaged` true → **every** file in the shape is eager, because the triage overlay reads a whole patch), `batchPaths(paths, max)`.
`wholePatch(status, bodyOf)` — the concatenation of the cached bodies in status-file order, or `null` when any file's body is missing; the one place a shape plus its bodies becomes a whole patch again.
`createFileDiffs({ deviceId, entityId, scope, call })` → `{ bodyOf(path), sync({ status, openPaths, triaged }), warm(status, { budget }), dispose() }`. `bodyOf` answers `{content_key, patch, truncated}` or `undefined`; a body whose `content_key` differs from the status file's is stale and refetched.

**C. The fold-aware keyed entry** — `spa/src/core/fileEntries.js` (new), rendering through `diffRender.js`
`export const COLLAPSED_PREVIEW_ROWS = 8;`
One view model, two constructors (the only place the two sources differ): `fileViewFromStatus(statusFile)` carries the wire `content_key`, and `fileViewFromParsedFile(parsedFile)` derives `contentKey = hashFileRows(parsedFile)` (`reviewMemory.js`), both → `{ path, status, add, del, contentKey, rows|null }`.
`fileEntry(view, { fold, body, ...options })` → `{ key: fileKey(view), html }`; `fileStackEntries(views, { folds, bodyOf, ...options })` → the array `patchList` takes through `createChangesetPaint`.
Collapsed: header (status, counts, "changed since your review" chip) + first `COLLAPSED_PREVIEW_ROWS` rows when a body is cached, else the expand affordance. Open: the full `diffFileHtml` body from the cached patch, or a short loading body. Html is byte-identical when `(contentKey, fold, body)` is unchanged. `git.show` changesets render through the same entry, bodies taken from the payload.

*The re-review chip is stamped and compared by `contentKey`* — `reviewMemory.js` keeps `stampChangeset` / `changedSinceChangeset` / `changesetStamped` as the one place that rule lives, but `stampReview(views)` now maps `path → view.contentKey` and `changedSinceReview` compares that, instead of calling `hashFileRows` over parsed rows. This is required by the view model: a collapsed uncommitted file has `rows: null` until its body is fetched, so a row-hash comparison would hash an empty row list and get the chip wrong (a false positive on every file after the first stamp, or never a chip at all). `hashFileRows` stays exported as the `contentKey` source for `fileViewFromParsedFile`, so `git.show` stacks and the uncommitted stack go through one comparison.

*Triage stays on the uncommitted stack.* `gitPane`'s `renderChangeset` passes `review: triageOverlay(patch)` for the uncommitted changeset on run surfaces today, and nothing here removes it: the overlay reads a whole patch (`triageModel`'s `hunkIdQueues` derives the hunk ids the pass speaks in from patch text, and a triage hunk carries only a `hunk_id`, never a path, so no subset of the patch can be selected from the pass alone). When a triage pass exists for the uncommitted changeset the stack is `triaged`: every file in the shape joins `pathsToFetch`' eager set, and the overlay's `patch` is `wholePatch(status, bodyOf)`. While that returns `null` (a body still in flight) the stack renders plain — the overlay is withheld rather than computed over a partial patch, which would mark named hunks untriaged. The review plug's changeset (kind `"diff"`) is untouched.

**D. Paint timing** — `spa/src/core/paintTiming.js` (new)
`export const SLOW_PAINT_MS = 100; export function timedPaint(name, paint)` — `performance.mark`/`measure` around the paint, `console.warn` naming the paint over the threshold. Wraps the changes-pane paint (spa-diffs) and `paintChat` (spa-chat).

**E. Chat entries, run bodies, and the fingerprint** — `spa/src/core/thread.js` + `spa/src/core/activityRuns.js` (new)
- `thread.js`: `export function timelineEntries(items, agentLabel, threadId, digests, { openRuns, runItemsOf })` → `{ entries: [{ key, html }], itemCount }`. Message key = its sequence, folded run key = its first sequence (still `data-activity-run`), lifecycle event key = its sequence. `timelineHtml`/`threadHtml` join the same entries — one builder.
- A collapsed run renders `activityRunSummary` head only, no children. An open run renders children from `runItemsOf(fromSequence)`: the tail run from the held window (live on every delta), a historical run from the cache or `thread.activity`.
- `activityRuns.js`: `export const ACTIVITY_RECORD_KIND = "activity";` `createActivityRuns({ deviceId, entityId, agentId, call })` → `{ itemsOf(fromSequence), open(digest), isOpen(key), toggle(key), openKeys() }` — a `Set` of open run keys plus `cachedBodies` over `thread.activity` paged at `limit: 200` until `has_more` is false.
- `thread.js`: `export function chatPaintFingerprint({ deliveredSequence, itemCount, digests, openRunKeys, fetchedRunKeys, selectedAgentId, agentLabel, sending, choiceState })` — the one place the paint's inputs are named. `paintChat` returns early on an unchanged fingerprint; `paintThreadKeepingPlace` still wraps the paint.

## Wire JSON (exact)

```jsonc
// git.status params: exactly one of {project_id} | {run_id} | {project_id, worktree_id},
//                    plus optional "if_status_key": "<16-hex>"
{ "branch": "build/ui-adjustments", "path": "/abs/worktree", "head": "b8156ab…" /* or null */,
  "repo_state": "clean", "upstream": "origin/main" /* or null */, "ahead": 0 /* or null */,
  "behind": 0 /* or null */, "stash_count": 0,
  "files": [ { "path": "spa/src/core/gitPane.js", "staged": "none", "index_status": "-",
               "worktree_status": "M", "content_key": "<hex|deleted>",
               "added": 12, "deleted": 3, "binary": false } ],
  "files_truncated": false,
  "stat": { "files_changed": 1, "insertions": 12, "deletions": 3 },
  "status_key": "<16-hex>" }
// no "patch", no "truncated". git.stage / git.unstage / git.commit["status"] answer this shape.

// git.status with a matching if_status_key — exactly this, nothing more:
{ "unchanged": true, "status_key": "<16-hex>" }

// git.diff params: <same scope> + "paths": ["a/b.js", "c/d.rs"]   (1..=50; more, or none, is an error;
//   a path outside the worktree, or an MCP config path, is an error — never a dropped path)
{ "files": [ { "path": "a/b.js", "content_key": "<hex|deleted>",
               "patch": "diff --git a/a/b.js b/a/b.js\n…" /* "" when unchanged */,
               "truncated": false } ] }

// thread.activity params: { "entity_id": "run-7", "agent_id": "agent-1"?,
//   "from_sequence": 120, "through_sequence": 870, "before_sequence": 400?, "limit": 200 }
//   limit default 200, max 500; unknown entity or a span outside the conversation is an error.
{ "items": [ { "type": "event", "data": { "sequence": 121, "event": "tool_use", … } } ],
  "oldest_sequence": 121 /* or null */, "has_more": true }
// items are oldest-first, same ThreadItem shape thread.page ships. No activity_digests here.
```

## IndexedDB records (`localCache.readCached/writeCached({deviceId, entityId, kind, sub})`)

| kind | sub | value | owner |
| --- | --- | --- | --- |
| `status` | `""` | the git.status answer as received (no patch to strip) | spa-diffs, cacheSync |
| `filediff` | file path | `{ content_key, patch, truncated }` | spa-diffs |
| `activity` | `String(from_sequence)` | `{ items }` — immutable per run | spa-chat |
| `log` / `show` / `diff` / `thread` / `tree` / `feed` / `surfaces` | as today | as today | unchanged |

Eviction stays entity-level through `evictEntity`; `filediff` and `activity` need no new eviction path.

## Corrections after the build (2026-09-07)

Three places where the code is right and the doc above was short of it:

- **IndexedDB `activity` record sub is agent-scoped:** `"<agent_id>:<from_sequence>"`, not `String(from_sequence)`. An entity can host several agents and each conversation numbers its items from one, so a bare `from_sequence` would let two agents' runs collide under one entity. Thread records already carry an agent sub.
- **Primitive 5, the bounded run read:** a gap read is three bounded statements deduped by sequence — the newest `run_items_shipped` rows (`THREAD_ACTIVITY_RANGE_SQL`), the run's oldest row (`THREAD_RUN_OLDEST_SQL`), and the run's newest tool call (`THREAD_RUN_LAST_CALL_SQL`) — so a stored page prints the same `last_tool_call` beside its count as a resident page does when the run ends in more than a cap's worth of thoughts. Cost: at most two rows more than the page ships.
- **Primitive E, the chat fold:** a folded run's `<details>` is render-owned through `domPatch.RENDERED_FOLD_ATTRIBUTE` (the per-run open set is the state; the reader-owned `<details>` carve-out still applies to every other fold). `runDigestToFetch` (activityDigest.js) is the one place the "which runs are fetched" rule lives, and it decides the live tail from what the client holds about the run (`data-activity-through`) as well as the digest; `activityRunKeyAt` (thread.js) is how a cross-surface reference finds the run a call is folded into.
- **git.diff carries an aggregate budget** (`GIT_DIFF_MAX_ANSWER_BYTES`, 4 MiB, spent in request order; files past it answer empty with `truncated: true`) because the relay refuses websocket messages over 8 MiB and encryption plus base64 adds a third.
