# Bridge Wire Protocol Spec

Status: draft 2026-09-13. Branch: `build/bridge-wire-interface`.

Two parts, one contract. Part 1 replaces the all-or-nothing push
invalidation with subscriptions that carry minimal payloads at a cadence the
client picks. Part 2 puts a semantic version on the API, a typed facade behind
it in the bridge, and version-selected adapters in the SPA, so a bridge and an
SPA that have drifted apart keep working. Part 2 step 0 must land before the
alpha cut; everything else follows on its own branch.

## Problem

The E2EE app RPC (`{id, method, params}` → `{id, ok, result|error}`, plus
`type`-tagged pushes) is pull-based, which is right. What is missing is the
policy layer that decides who hears about a change and how often, and any way
for the two ends to tell each other which protocol they speak.

Today's push is `ChangeBus` (`bridge/src/changes.rs`): `board.changed` and
`entity.changed{id}`, content-free, coalesced over a 250 ms window with a 1 s
per-entity settle. `session.hello` subscribes a session to every event for
every entity on the device; the `close` frame unsubscribes. There is no
scope, no kind, no interval, no off switch, and no way to say which workspace
the human is looking at.

The SPA (`spa/src/core/changeEvents.js`) treats an event as "run the
surface's poll callback". A mounted surface polls at 1.6 to 2 s, stood down
to a 60 s safety poll once push is armed; `core/cacheSync.js` refreshes every
entity in the feed's active set on a flat 60 s loop. Push and poll end in the
same paint, which is sound, but there is no tier to hang "30 s for background
git, minutes for background files" on.

On versioning: the transport envelope carries `PROTOCOL_VERSION = 1`
(`transport.rs`), two sub-contracts carry their own `version: 1`, and the
crate has a Cargo version. The API itself has none. A client detects features
by whether a method is refused. An unknown method is the free-text string
`"unknown method: x"`. The alpha ships tomorrow; a bridge shipped without an
API version is one no later SPA can ever identify.

## Findings

1. **No file or git change source.** `diff::watch` (`bridge/src/diff.rs`,
   `DiffWatcher`, built on `notify`) has one caller: its own unit test. Git
   stat, external scan, and workspace summary are recomputed on read behind
   10 s TTLs (`app/board/cache.rs`, `TASK_STAT_TTL`, `EXTERNAL_SCAN_INTERVAL`,
   `WORKSPACE_SUMMARY_TTL`), stale-while-revalidate. A file an agent writes
   reaches the wire only when a client read happens to trigger a refresh
   whose stat differs (`publish_diff_refresh` → `note_entity_settled`). With
   nobody reading, nothing is pushed. `fs.write` is the one immediate path.
2. **Subscription is all-or-nothing and broadcast.** Every subscriber gets
   every `entity.changed`, individually encrypted per session per key
   (`changes.rs`, `flush`). N browsers watching M entities cost N×M frames
   per window.
3. **Cadence is fixed.** `DEFAULT_COALESCE_WINDOW` and `ENTITY_SETTLE_WINDOW`
   are process constants. `coalesce_window_ms` in the greeting is advisory;
   the only setter is `#[cfg(test)]`.
4. **Events carry nothing a client can compare.** `ChangeKey` is
   `Board | Entity(id)`. A client cannot tell thread from git from files, and
   cannot skip a refetch it already has, though every read already has a key
   to compare against: `status_key`, `diff_key`, `content_key`, thread
   `sequence`, `choice_revision`, file `revision`.
5. **Priority stops at the client.** `core/readRequests.js` holds background
   reads until the foreground is quiet, but nothing crosses the wire. The
   dispatcher (`carrier/dispatch.rs`, `DISPATCH_WORKERS = 8`,
   `DISPATCH_QUEUE_DEPTH = 256`) is FIFO; a burst of background pulls queues
   ahead of the focused surface's read.
6. **No API version, no error codes, no contract fixtures beyond one.**
   `fixtures/chat_operation_contract.json` is the one shape both ends test
   against (`bridge/src/operation.rs`, `spa/test/chatRepository.test.js`).
   The rest of the ~140 verbs are ad hoc `json!` in the handlers.

## Design

### Principles

- **The wire carries keys, not bodies.** A push says what moved and the key
  it moved to. The client pulls only when the key differs from what it holds.
  Terminal output keeps its own path (`term.output`); it is not a change.
- **Cadence is the subscriber's choice.** The bridge never decides that a
  workspace is "background"; the client says so by the mode it subscribes
  with. One entity may be covered by several subscriptions at different
  cadences; the client's key comparison dedupes.
- **Foreground never waits behind background.** Priority rides the request
  envelope and the subscription, and the dispatcher honours it.
- **Neither end is ever assumed current.** The bridge serves the current API
  major and the previous one. The SPA carries an adapter per supported major
  and picks by the version the bridge reports. A gap in either direction is
  a gate view with an update instruction, never a broken surface.
- **Additive within a major.** An unknown field in a reply or push is
  ignored; an unknown top-level param on a request is refused by name (since
  1.24.0, see step 2.2), the way an unknown push kind refuses its
  subscription. Unknown methods and unknown event types are errors and no-ops
  respectively, never crashes.
- **Content-free stays a rule for the api path only.** `changes.rs` keeps
  events content-free "like every other signal Build sends about work it
  cannot read". That binds the web-push path through skriftapp, which is
  plaintext to the server (`notify.rs`). It does not bind the E2EE session;
  the relay never sees inside. File paths and keys may ride the session.

### Part 1: push subscriptions

#### Step 1.1: the wire

Three additive verbs and one event. Old clients keep the legacy events until
they call `changes.subscribe`; see step 1.5.

`changes.subscribe` (upsert by `subscription_id`; re-sending with a new mode
is how a client changes cadence):

```json
{
  "subscription_id": "s-focus",
  "scope": { "kind": "entity", "id": "run-7" },
  "kinds": ["state", "thread", "git", "files"],
  "mode": "realtime",
  "priority": "foreground"
}
```

- `scope`: `{"kind":"board"}` (feed-level state only), `{"kind":"entity",
  "id"}` (one issue, run, or worktree), or `{"kind":"all"}` (every entity
  the board currently lists, tracked as the board changes).
- `kinds`: any of `state` (lifecycle, agent liveness, attention), `thread`
  (conversation items), `git` (status, index, HEAD, refs), `files` (working
  tree paths). `board` scope accepts only `state`.
- `mode`: `"realtime"` (flushed on the bus's 250 ms window, first change on
  an idle subscription goes at once), `{"batch_ms": N}` with N clamped to
  `[1000, 600000]`, or `"off"` (kept, delivers nothing; a later upsert turns
  it back on without a gap in what it will report).
- `priority`: `"foreground"` or `"background"`; orders flushes (step 1.3)
  and stamps the pulls the client makes in response (step 1.4).

Reply: `{"subscription_id", "watch": "live" | "polled"}`. `polled` means a
worktree in scope could not get a filesystem watcher (step 1.2) and its
`git`/`files` kinds come from the TTL refresh instead.

A kind this bridge does not know refuses the whole subscription with
`invalid_params`, `error: "Build cannot subscribe to <kind>: this bridge does
not know it."` ("<a> and <b> … them" for several), and `details: { "kinds":
[<kind>, …] }` naming every unknown kind in the order asked (since 1.24.0,
announced as `changes.refusedKinds`). Nothing of the refused request is
subscribed. A client drops the named kinds and re-subscribes once with the
rest; it still asks only for the kinds the greeting advertises.

`changes.unsubscribe {subscription_id}` → `{"ok": true}`.
`changes.list` → the session's subscriptions, for the SPA's reconnect diff
and for `bridge.stats`.

The event, one frame per subscription per flush:

```json
{
  "type": "changes",
  "subscription_id": "s-bg",
  "items": [
    {
      "entity_id": "run-7",
      "state": { "run": "working", "agents": 1, "attention": "none" },
      "thread": [{ "agent_id": "agent-3", "last_sequence": 412 }],
      "git": { "status_key": "9f3c1a0b7e2d4c55", "head": "a1b2c3d" },
      "files": { "paths": ["src/lib.rs", "src/app/rpc.rs"], "truncated": false }
    },
    { "entity_id": "board", "state": { "revision": 1183 } }
  ]
}
```

Only kinds that moved appear on an item. `files.paths` is deduped, relative
to the worktree root, capped at `FILES_PER_FLUSH = 200` with `truncated:
true` past that (a truncated list means "refetch the tree", not "these
paths"). `state.revision` on the board item is a monotonic counter bumped by
every `note_board`; a client that holds the same revision skips `board.list`.
`git.status_key` is the same FNV-1a-64 key `git.status` answers with, so the
comparison is against the cached `status` record with no translation.

#### Step 1.2: the producer

`bridge/src/watch.rs` (new): `WorktreeWatcher::start(worktree_root,
entity_id, sink) -> Result<WorktreeWatcher, WatchError>` over
`notify::recommended_watcher`, recursive. Raw events are classified, not
recomputed:

- A path under `.git/` notes the `git` kind for the entity. `index.lock`,
  `*.lock`, and `objects/` churn are dropped; `index`, `HEAD`, `refs/`,
  `MERGE_HEAD`, `logs/HEAD` count.
- Any other path notes the `files` kind with that path, and the `git` kind
  (a working-tree edit changes status too).
- Paths matched by the repository's ignore rules (`ignore` crate walk of the
  root's `.gitignore`, `.git/info/exclude`, global excludes) are dropped from
  `files` but still note `git`; a `node_modules` write is not a change the
  human reads, but it can move status.

The sink is `ChangeBus::note_files(entity_id, paths)` and
`ChangeBus::note_kind(entity_id, Kind::Git)`: leaf-lock inserts, exactly as
`note` is today. No git work runs in the watcher thread.

One watcher per worktree that at least one subscription covers with `git` or
`files`. Started on the subscribe that first covers it, on a blocking task;
dropped when the last such subscription goes or the entity leaves the board.
`{"kind":"all"}` covers every board worktree, so a background subscription
puts a watcher on each. If `start` fails (inotify watch limit, a checkout on
a filesystem `notify` cannot follow), the bridge logs once per worktree,
answers `watch: "polled"`, and the TTL refresh keeps serving that worktree.
The refresh path's `note_entity_settled` is rewired to note the `git` kind,
so it feeds the same subscriptions.

`diff::watch` and `DiffWatcher` are deleted. Recomputing a whole
`WorktreeDiff` per burst is the cost this design exists to avoid.

#### Step 1.3: the bus

`ChangeBus` keeps its shape (leaf lock for notes, one flusher task, no app
lock) and gains subscriptions:

```rust
struct Subscription {
    id: String,
    session: SessionSender,
    scope: Scope,
    kinds: KindSet,
    mode: Mode,          // Realtime | Batch(Duration) | Off
    priority: Priority,
    pending: BTreeMap<EntityId, PendingItem>,
    last_flush: Option<Instant>,
}
```

`note_*` resolves the entity to every subscription whose scope covers it
and whose kinds include the kind, and inserts into that subscription's
`pending`. Coalescing is therefore per subscription: a thousand notes of one
entity are one item, and a subscription that was `off` for an hour holds one
item per entity, not an hour of history. `PENDING_KEY_CAP` applies per
subscription and collapses to a bare board item as today.

The flusher's next deadline is the earliest over all subscriptions:
`first_pending + 250 ms` for realtime, `last_flush + batch_ms` for batch.
Foreground subscriptions flush before background ones in the same turn.

**Amended (stage 3): batch mode is leading-edge with a cooldown.** As
written above a batch subscription made the reader wait out the whole
window before its first word — thirty seconds of nothing on a workspace
that had just moved. A batch subscription that has not flushed inside
`batch_ms` flushes AT ONCE, and the window is the floor under the next
flush rather than the delay before the first: the first change after a
quiet spell goes immediately, and a burst behind it is held back and
coalesced until the cooldown is up. The greeting reports the semantics
(`changes: { batch: "cooldown" }`) so a client can tell a bridge that
leads from one that lags.
`ENTITY_SETTLE_WINDOW` stays as the floor for realtime `git` and `files`
items: an agent writing a file a second costs one item a second, not one per
write.

Flushing computes what the item carries, off the app lock, on the flusher's
own `spawn_blocking`: `status_key` via `gitgui::status::status_shape`
(made `pub(crate)`; it already returns the key beside the shape) for each
pending entity with a `git` item, `head` from `refs`, `last_sequence` from
the thread's resident tail (a read under the app lock, held for the lookup
only). A batch subscription covering thirty worktrees at 30 s therefore
costs thirty status walks per 30 s, only for the worktrees that moved. That
is the "git status every 30 seconds" the design asks for, and it is bounded
by what changed rather than by what exists.

A `SessionSender` whose push fails drops every subscription of that session,
as one failing push drops the subscriber today. The `close` frame does the
same.

#### Step 1.4: request priority

The request envelope gains an optional `"priority": "background"` beside
`id`, `method`, `params`. Absent means foreground. `carrier/dispatch.rs`
keeps two queues in front of the same eight workers; a free worker takes
foreground first. At most `BACKGROUND_WORKERS = 2` run background frames at
once, so six workers are always available to the focused surface no matter
how many background pulls are waiting. The fold (`COALESCED_READ_METHODS`)
is unchanged and folds across priorities: a background `git.status` that
matches a queued foreground one is answered by it.

`FrameTimer` records priority; `bridge.stats` reports the two queues' depths
and p95 separately, so a slow foreground under load is visible as a bridge
problem and not a design one.

Per-terminal lanes are unaffected; terminal frames never carry the field.

#### Step 1.5: legacy and the greeting

`session.hello` gains `"changes": "legacy" | "subscriptions"`, default
`legacy`. Legacy is today's behaviour: the session subscribes to the
board-scope, all-entity, realtime `board.changed` / `entity.changed` events.
`subscriptions` means the session hears nothing until it calls
`changes.subscribe`. The greeting reports `"events": ["board.changed",
"entity.changed", "changes"]` and `"changes": {"subscriptions": true,
"kinds": [...], "batch_ms": {"min": 1000, "max": 600000}}` so an adapter can
tell (Part 2) without probing. A session that calls `changes.subscribe`
while in legacy mode is switched to subscriptions on that call and its
legacy subscription is dropped.

#### Step 1.6: SPA

Rewritten after the fact (the cache-first overhaul). What this step
originally described — a subscription manager beside a set of poll
callbacks, a 60 s safety poll under every mounted surface, and a cacheSync
that pulled on a miss — is not what shipped. What shipped is stronger: the
client has no polls to stand down, because it has no views that read the
wire.

**The cache is the only thing a view reads.** Every surface subscribes to
cache addresses and repaints when one changes. No view calls the bridge for
a read. The sync layer is the only reader of the wire, and it writes what it
reads.

**Three subscriptions per device**, on `changes.subscribe`:

| id | scope | kinds | mode | what it feeds |
| --- | --- | --- | --- | --- |
| `s-inbox` | `all` | `state`, `thread` | realtime | inbox rows, every conversation in every workspace |
| `s-background` | `all` | `git`, `files`, `terminals` | `{batch_ms: 30000}` | background workspaces' git and file surfaces |
| `s-active` | `entity` = the routed workspace | `git`, `files`, `terminals` | realtime | the workspace the reader is standing in |

`s-active` is re-issued on every route change. The project page has no
entity subscription: it is a filtered inbox.

**Sync is ordered and bounded, never full.** On boot, on reconnect and on
tab return the sync layer reads, in this order: device list, project list,
workspace list, board; then for the active workspace and then every other
active-or-recent one: status, tree root, terminals, conversation list, and
last the two cursored reads (step 1.8) — commits since the newest cached
hash, and per conversation the items after the cached sequence. Everything
not cursored is a wholesale replacement of a small shape.

**No client poll.** There is no safety poll and no 60 s loop. A
registration that names a cadence is refused
(`core/changeEvents.js` `watchChanges`). The one remaining timer in the
client is the account's device-presence read against the skriftapp API,
which is not this bridge and has no push path.

**Cache lifetime.** Device, project and workspace list entries have no TTL
and leave only when the list stops naming them. Per-workspace data is
dropped at once on Done or Delete, and `WORKSPACE_DATA_TTL_MS` after its
last write once the workspace is only Recent. Active workspaces never
expire.

**Deeper file listings are re-listed, not pushed.** The bridge cannot know
which directories a reader has walked into. The `files` item carries the
changed paths and the root listing; the client re-lists only the held
directories a changed path sits in.

**Optimistic writes go into the cache.** A sent message is written under a
provisional key and replaced when the push carries the real item with its
sequence; a row a press moved is written into the row's record and the
board's list, and the `state` push confirms it. Views never hold a second
store.

Legacy `board.changed` / `entity.changed` are still sent by the bridge and
no longer read by this client: a hint is news a view would have to go to the
wire to act on. Removing them from the bridge is its own commit, later.

#### Step 1.7: items carry bodies (stage 2)

Written after the fact: what step 1.1 designed as a hint — "this entity
moved, go and read it" — ships as a body. A hint is a read a view has to
make, and the client this protocol serves has no view that reads the wire.
So every item carries what the surfaces showing that entity paint from, and
the client writes it into its cache and repaints.

| kind | what the item carries |
| --- | --- |
| `state` | the whole feed row, exactly as `board.list` lists it |
| `thread` | one tip per conversation: the items since this subscription's last flush, the sequence they run from, and the conversation's total |
| `git` | `status`, the latest commits, the unpushed commit list, and the working-tree diff's key and size (step 1.9) |
| `files` | the changed paths and the root listing |
| `terminals` | the tab list (a new kind; a console is a surface like any other) |
| the board item | the entity ids that left the board, and the project and workspace lists when they moved |

Two things never ride an item: a file's contents, and the patch behind one
commit. Both are read on demand by the reader who opened them.

The board item carries deltas, not the board: each moved entity's row rides
its own `state` item, and the board item says which entities left. Pushing
the whole `board.list` on every change was rejected — it re-sends every
row's stat for one row's move.

Thread items are per-subscription deltas. The bus already holds
`last_flush` and `pending` per subscription; it gains, per subscription, the
last sequence emitted per agent, so a flush carries the items after it. No
client state on the bridge, and no durable log.

The thresholds both sides are written against:

| name | value | where |
| --- | --- | --- |
| `LATEST_COMMITS` | 20 | `git.log` default when `since` is unknown |
| `LATEST_THREAD_ITEMS` | 100 | `thread.page` forward read cap |
| `THREAD_PUSH_MAX_ITEMS` | 100 | a thread item past this carries the tip only |
| `WORKING_TREE_DIFF_MAX_BYTES` | 262144 | the `run.diff` / `worktree.diff` body cap; since step 1.9 no size rides a push |
| `UNPUSHED_COMMITS_MAX` | 20 | commits whose patches the client syncs |
| `COMMIT_PATCH_MAX_BYTES` | 262144 | `git.show` `max_bytes` |
| `RECENT_FILES` | 5 | file contents the client keeps per workspace |
| `FILE_MAX_BYTES` | 1048576 | larger files are not cached |
| `BACKGROUND_COOLDOWN_MS` | 30000 | the background tier's cooldown |
| `WORKSPACE_DATA_TTL_MS` | 259200000 | client-side eviction |

#### Step 1.8: cursored reads (stage 1)

The reads the client makes past its first are deltas, so coming back to a
workspace costs what moved rather than what exists:

- `git.log` takes `since` — a hash the client already holds — and answers
  the commits after it, or the latest `LATEST_COMMITS` when the hash is not
  in this history (a rebase, a reset) with `reset: true` saying so.
- `thread.page` takes `after_sequence` and answers forward from it, capped
  at `LATEST_THREAD_ITEMS`. A forward read says nothing about the end of the
  list: `has_more` on such a page is about the window it walked.
- `git.show` takes `max_bytes` and answers `truncated: true` rather than a
  quarter of a megabyte the reader did not ask for.

#### Step 1.9: keys, not bodies (stage 3)

Written after the fact, from a phone. Step 1.7's rule — an item carries what
the surface paints from — put megabytes on a relayed path in the first two
seconds of a session: one connect measured 2.28 MB on the app channel, of
which `git.unpushed`'s patches and the whole-branch `run.diff` were 1.5 MB,
for surfaces nobody had opened. On a phone-class path everything else on that
one SCTP association waits behind them, including the 3 s liveness ping, and
the session was torn down and re-minted before the first paint.

So the largest bodies stop riding anything unsolicited:

- a `git` item sends `"diff": null` and `diff_bytes`, whatever the size. The
  item still names the diff — `diff_bytes` says it moved and how big it is —
  and `run.diff` / `worktree.diff` answer the body when a reader opens the
  changes. This is the shape a diff past `WORKING_TREE_DIFF_MAX_BYTES`
  already had, so no client learns a new case.
- `git.unpushed` takes `patch: false` and answers the commit list, the base
  and the `diff_key` with no patches. `true` is the default, so review and
  commit surfaces are unchanged; the client's cold pass asks for `false`.
- `run.diff` / `worktree.diff` / `project.diff` take `patch: false` for the
  same reason: the pass wants the key and the per-file stats, not the hunks.

The rule this step is under, and the one that bounds it: **every body a push
stops carrying must remain fetchable on demand by the surface that shows it.**
A key with no verb behind it is a blank pane, not a saving.

#### Step 1.10: a receipt on admission

A request's deadline is the client's only protection against a path that
died, and the client cannot tell a request that never arrived from one
queued behind eleven agents' work. It guesses — and a 15 KB attachment was
reported as failed after 10 s while the bridge was storing it.

So the intake receipts every request the moment it is admitted, before
anything decides how long answering will take:

```json
{ "id": 41, "accepted": true }
```

It rides the carrier the frame arrived on, carries the request's id and
nothing else, and is sent ahead of dispatch. `ok` is deliberately absent:
that field is what says a reply has settled a call, and a receipt settles
nothing. A reader that treats every frame bearing its id as the answer would
settle the call on the receipt — so `accepted` with no `ok` is the test, and
the contract fixture (`fixtures/api/v1/events.json`) carries it.

What it buys the client: the deadline before the receipt is about the PATH
and stays short; after it, the client knows the device holds the request and
waits far longer for the answer. A posted message can read *queued* from the
receipt and *delivered* from the reply.

#### Step 1.11: hunks per file (stage 3, second half)

Step 1.9 stopped the bodies riding pushes; what was left was the reader's
own. Landing straight on a workspace measured 1076 KB on the app channel, of
which 1015 KB was one `run.diff` — the whole changeset, fetched because the
review surface drew itself from one patch string.

So the review surface reads what the rest of the client already reads:

- every diff's file rows carry `additions`, `deletions` and `content_key`
  beside `path` and `status`. The counts are taken per delta in the same
  print the roll-up is taken in, and the key is an FNV-1a over that one
  file's patch text as it prints, so both cost nothing beyond the walk. A
  stack drawn from rows alone has its `+`/`−` and its review bar from the
  counts, and its re-review chip from the key.
- `git.changeset_diff` (1.4.0) takes the ordinary `git.*` scope and `paths`
  (1 to 50, the cap `git.diff` already has) and answers the same changeset
  narrowed to those paths: `{stat, files, patch, file_edited_at, diff_key}`.
  Which changeset a scope names follows its whole-patch verb — `run_id` the
  run against its baseline, `project_id` + `worktree_id` a checkout against
  its merge base, `project_id` alone the project's uncommitted work. The
  `diff_key` is the WHOLE changeset's, because that is what says whether a
  held body still stands.
- the whole-patch verbs are unchanged. Review and commit still read them,
  and `patch: false` (step 1.9) is what a surface asks when it means to
  fetch per file.

Client side: the review plug (`spa/src/core/changesReview.js`) paints a view
per row with no hunks in it and fetches bodies through
`spa/src/core/changesetBodies.js` for the files `viewport.shouldLoad` says
are on screen or expanded — the same gate the git pane has always fetched
its uncommitted bodies through, and the same per-path cache discipline
(`core/fileDiffs.js`). A reader who opens the changes pane and never scrolls
pays for the first screenful; a reader who never opens it pays nothing.

### Part 2: API versioning

#### Step 2.0: report a version, before the alpha

`bridge/src/api/mod.rs` (new): `pub const API_VERSION: &str = "1.0.0";`.
`session.hello` and `ping` both reply with `"api_version": API_VERSION`.
`session.hello` accepts an optional `client` object, `{"name": "spa",
"version": "<git sha>", "api_range": ">=1.0.0 <2.0.0"}`, recorded per
session and counted by `bridge.stats` so a later decision to drop a major is
made from numbers. Nothing else changes. This is the whole of step 2.0 and
it ships with the alpha.

The alpha bridge is `1.0.0`. A bridge that reports no `api_version` is
treated by the SPA as `0.0.0`, pre-alpha, and handled by the v1 adapter with
today's method-refusal probing; that path is deleted once no `0.0.0` bridge
has been seen in `bridge.stats` for a month.

#### Step 2.1: semver rules

- **Patch**: no wire change.
- **Minor**: additive. New methods, new optional params, new fields on
  results and events, new event types, new error codes, new enum values on
  fields documented as open. A client must ignore what it does not know.
  Part 1 is `1.1.0`.
- **Major**: a method or field removed or renamed, a param made required, a
  field's meaning or type changed, an enum documented as closed extended.
  The `plan.*` aliases over `issue.*` (`app/rpc.rs`, `alias_param`) are the
  kind of thing a major retires.
- **Release cadence**: one minor number covers all additive wire changes in
  a release. Do not advance the minor separately for each new verb, field, or
  capability. A breaking change advances the major.
- The bridge serves the current major and the previous major until the
  later of 90 days after the new major ships or `bridge.stats` showing no
  session on the old one for 30 days.
- `PROTOCOL_VERSION` (envelope), the MCP protocol date, and the Cargo
  version stay separate. They version different things.

**One compatibility rule is not about versions at all, and every reader of
this wire needs it: a frame bearing your request's id is not necessarily the
answer.** Since step 1.10 the intake receipts a request on admission, so an
id comes back twice — once as `{"id", "accepted": true}` and once as the
reply. `ok` is what says a reply settled a call; `accepted` with no `ok`
settles nothing. A reader that treated the first frame carrying its id as the
answer settles every call on the receipt: in this repo that was three readers
(`spa/src/core/sessionRpc.js`, `web/peer.mjs`, `bridge/src/rtc/testing.rs`),
and outside it, it is whatever anyone has written against this wire. It rides
no version gate on purpose — a client that cannot be taught is a client that
must not be sent receipts, and there is no such client.

#### Step 2.2: the bridge facade

`bridge/src/api/v1/` owns the wire. One module per verb family (`board.rs`,
`thread.rs`, `git.rs`, `changes.rs`, …), each holding typed request and
response structs:

```rust
#[derive(Deserialize)]
pub struct GitStatusParams {
    pub scope: Scope,
    #[serde(default)]
    pub if_status_key: Option<String>,
}

#[derive(Serialize)]
pub struct GitStatusResult { … }
```

A request param the verb's type does not declare is refused, never dropped
(since 1.24.0, announced as `params.strict`). Before it, the facade parsed
params into the typed struct and handed the implementation that struct
serialised back, so a field this bridge did not know vanished and the verb
answered `ok`: `issues.create` with an `assignee` a bridge predated filed the
issue unassigned, and a `patch: false` on a `project.diff` that did not
declare it answered the whole patch. The rule, in `parse_params`:

- A top-level param the params type does not declare is refused with
  `invalid_params`, `error: "unknown param: <name>"` (`"unknown params: <a>,
  <b>"` for several, sorted), and `details: { "params": [<name>, …] }`. The
  whole request is refused, as an unknown kind refuses the whole
  `changes.subscribe`; nothing of it runs.
- "Declared" is what the type reads: its fields, a flattened scope's fields,
  a `rename` or `alias`. A declared field at its default (`null`, `false`) is
  declared. `deny_unknown_fields` cannot say this — serde does not support it
  with `#[serde(flatten)]`, which every scoped verb uses — so `parse_params`
  asks the type: a field whose value, swapped for a probe, changes nothing the
  type holds is one it never read.
- The rule binds a verb's top-level params. Nested objects keep their own
  rules: a message context item's unknown field is ignored and its unknown
  `kind` refused (step 2.6); a free-form value (`issues.create`'s `assignee`)
  is checked by the verb that reads it.
- So a new param is an addition like any other: it joins the release's minor
  and, when its verb already exists, a feature name. A client sends it only
  to a bridge whose greeting announces it. An older bridge (before 1.24.0)
  still drops what it does not know, which is why the gate is the greeting
  and not the refusal.
- `fixtures/api/v1/` holds every verb to it: `bridge/tests/api_contract.rs`
  adds a field no verb declares to each fixture's params and expects the
  refusal to name it.

Responses
derive `Serialize`; `skip_serializing_if` for optional fields so a field's
absence and its `null` mean the same to every client. The `route` table in
`app/rpc.rs` becomes `api::v1::dispatch(method, params) -> Result<Value,
ApiError>`, which parses params into the typed struct, calls the handler,
and serialises the typed result. A handler that returns `serde_json::Value`
is a lint failure (`clippy::disallowed_types` for `Value` in `api/v1`
signatures). An implementation change that alters a shape is now a change
to a type in `api/v1`, which is a change to a fixture (step 2.3), which is
a version bump.

The QA stream verbs (`stream.start`, `stream.events`, `stream.state`) are
not part of `api/v1`; they move behind `BRIDGE_QA_AGENT=1`.

#### Step 2.3: contract fixtures

`fixtures/api/v1/` holds one file per method and one for events:

```json
{
  "method": "git.status",
  "since": "1.0.0",
  "params": { "scope": { "kind": "run", "id": "run-7" }, "if_status_key": "9f3c1a0b7e2d4c55" },
  "result": { "unchanged": true, "status_key": "9f3c1a0b7e2d4c55" },
  "errors": ["not_found", "invalid_params"]
}
```

`fixtures/api/versions.json`: `{"current": "1.1.0", "supported_majors":
[1], "deprecated": []}`.

Bridge test (`bridge/tests/api_contract.rs`): every fixture's `params`
deserialises into its typed struct and every `result` round-trips through
the typed result byte-for-byte after canonical ordering; a method with no
fixture fails the test; a fixture with no method fails the test.
`API_VERSION` must equal `versions.json`'s `current`.

Since 1.22.0, `session.hello` also returns a flat `capabilities` array of
strings. Every served verb appears under its exact method name (for example,
`issues.attach`); a QA-only verb appears only on a QA bridge. Cross-verb
behavior or response shapes use feature names: `changes.subscriptions`,
`requests.priority`, `errors.codes`, `diffs.perFile`,
`issues.attachments`, `issues.context`, `issues.watching`, `conversations.settings`,
`changes.bodies`, `requests.receipts`, `messages.context`,
`threads.postOperations`, `settings.roleModels`, `settings.projectAgent`,
`agents.names`, `messages.fromAgent`, `issues.doneSinceLeft` (`done_at` on
issues, `user_session` on `issues.list`, and the `user.present` verb, see the
Issues spec), `messages.issueNotices`, `board.usageLimits`,
`threads.newestDeltaPagination`, `issues.commentUserMentions`,
`issues.agentIdentities`, `issues.attachmentChunks`,
`issues.commentUserNotifies` (`notifies_user` on issue comments, and the
narrower "needs you" rule it makes possible; see the Issues spec),
`issues.createdUserMentions` (`mentions_user` on an agent's `created` issue
event, so an issue filed with `create_issue` and `mention_user: true` needs the
user until that event is read; see the Issues spec),
`params.strict` (an undeclared param is refused; step 2.2),
`workspaces.lifecycle` (see below),
`branches.finishDelete` (`branch.finish` honours `action: "delete"`),
`workspaces.reclaimBranches` (`workspace.reclaim` deletes the workspace's local
branches, see below), `settings.workspaceLifecycle` (the reclaim service's
idle threshold and prune switch on `settings.*`, see below),
`issues.listPaged` (`issues.list` pages; see below), and `bodies.pages`
(body reads in byte ranges; see below). The method registry
supplies typed verb names, and a small explicit list supplies legacy and
session-scoped verbs. Contract tests require every method fixture to have an
announced name and compare an actual `session.hello` reply with that
registry. The existing `events` array and all earlier greeting fields remain
present.

`github.repos` (since 1.23.0, gated on its verb name) takes no params and runs
`gh repo list` for the signed-in account and for each organisation `gh org
list` names, answering the union as `{ repos: [{ name_with_owner,
description?, ssh_url, url, private, pushed_at? }] }` within 20 s. An
organisation whose listing fails is left out. When `gh` is missing, signed out,
slow or failing, the refusal's `error` is a sentence the UI shows as is
("Build cannot list GitHub repositories on <machine> because gh is not
installed."). The bridge keeps nothing: the SPA caches and searches the list.

`branch.finish` with `action: "delete"` (since 1.24.0, announced as
`branches.finishDelete`) is Done and then deletes the local branch the finish
resolved the workspace by, in each source repository its directories carry it
in. It is refused before anything is removed (`conflict`, "Build cannot delete
the branch <name>: it is a default branch." for `main`, `master`, the branch a
remote's `HEAD` names or the project's base, "…: it is checked out at <path>."
— or being rebased or bisected there — or "…: it has commits no remote has.")
and measured again once the checkout is gone, when each remote is also asked
which branch its `HEAD` names, since what the repository remembers of it can
be missing or stale ("…: Build could not confirm it is not the remote's
default branch." when one cannot be asked). The delete names the commit
those checks passed, so a branch that gained commits in between is kept, and
the checkouts are read again immediately before and after it, so a checkout
that moved onto the branch keeps it. A branch that stayed then is
`branch_deleted: false` with `branch_reason`, beside the workspace's removal,
which stands. Success adds `branch_deleted: true` to the answer and a
`branch_deleted` event (payload `branch`, `workspace_id`) to every issue
linking the workspace or the branch. If a checkout moves onto the branch
between the last check and the atomic delete, Build restores the measured tip
with a create-only ref write. It retries once when a failed write leaves the
ref absent. If the ref is still absent, `branch_deleted: true` carries
`branch_reason` with the measured commit, repository and checkout details;
the linked `branch_deleted` event carries the same `reason`. A concurrent
ref at a newer commit is left untouched and reported as a refusal.
Absent or any other action keeps the branch. A bridge that does not announce
the name keeps the branch whatever the action says, so a client must not
promise the deletion to one.

`bodies.pages` (since 1.26.0, #95) announces `range` on the body reads:
`fs.read`, `git.diff`, `git.show` and `git.changeset_diff`. `range` is
`{offset, bytes}`: the byte offset the page starts at (0, or the `end` of the
page before) and the most it may carry, clamped to 4 KiB..1 MiB. The answer
carries the page where the whole body would ride (`content_b64`, `patch`) and
`range: {offset, end, total}`: where the page's bytes sit in the whole body,
`end` being the next page's offset and equal to `total` on the last. A page
ends after its last line end, so each is whole lines a client paints alone;
only a line longer than a page is cut, at a character boundary. An offset past
the end answers an empty page at the end. On the patch reads (`git.diff`,
`git.show`, `git.changeset_diff`), whose bodies are text, an offset inside a
character (the end of a page of an earlier version of the patch) answers from
that character, under the version it was cut from, so the client sees the
body moved. `fs.read` pages any bytes, text or not, and reads from the offset
asked for exactly; a page of a changed file names another version all the
same.
`git.diff` and `git.changeset_diff` take exactly one path beside `range`
(`files[0]`, or the top-level `patch`, is that path's page, under the same
`content_key` and `diff_key` a whole read answers); `git.show` refuses `range`
beside `max_bytes`, and still answers the exact `stat` and `patch_bytes`. An
`fs.read` page is never `editable` and names no `revision` (it is not the
file), and its `range.version` names the file's modification time and size
(and on Unix its change time and inode), the same after the page's bytes were
read as before, so a file rewritten under a read is read again;
a patch page's `range.version` is a digest of the whole patch, which moves
with HEAD and the merge base even where the file's `content_key` does not.
Pages whose versions differ were cut from different contents. A ranged answer
is never `truncated`. Offsets are bytes rather than lines because the bridge
seeks a file to one without reading what comes before it, and the whole
body's size is known before the first page. A bridge that does not announce
the name refuses `range` as undeclared, so a client sends it only to one that
does.

`issues.listPaged` (since 1.25.0, #85) announces `limit` and `cursor` on
`issues.list` and `next_cursor` in its answer. `limit` is 1 to 500 and asks for
at most that many issues, in the list's own order (number descending); absent,
the list is whole, as it always was. A page with more after it carries
`next_cursor`, an opaque string; sent back as `cursor` with the same filter, it
answers the issues numbered below the last one the page held. Numbers never
move, so an issue filed between pages lands above the first page (where the
next read from the top finds it) and never moves a row across a cursor; an
issue that leaves the filter between pages is simply not on the next one. A
page can hold fewer than `limit` issues, or none, and still carry
`next_cursor`: `assignee` and `label` are not columns the store can seek on,
so a page reads at most four rows for each it may answer and, when those run
out with rows still below, stops short and names where the next page starts.
A label nobody carries costs a bounded read per page rather than the whole
project. A client keeps asking while `next_cursor` is present and stops only
when it is absent, never on a short or empty page. A
cursor names the list it was made in — the bridge's store, by a key the store
mints once and keeps, and the project, by its repository path rather than its
boot-local `proj-N` — and a cursor from another store or another project is
refused (`invalid_params`, "Build cannot continue this list: the cursor was
made for another project or on another device."); it survives a restart of the
bridge that made it. It names the filter it was made under too — `state`,
`status`, `assignee` and `label`, compared as they mean, so a column's display
name and its slug are one filter — and with any other filter it is refused
("Build cannot continue this list: the cursor was made for a different
filter."). A cursor this bridge cannot read is refused as "Build
cannot read this cursor: ask for the list again from the start.", and a limit
out of range as "Build cannot list 0 issues at a time: a page holds 1 to 500."
Every page carries `project_id` and `user_session` like the whole list. A client
sends `limit` and `cursor` only to a bridge that announces the name
(`params.strict` refuses them elsewhere). `fixtures/api/v1/issues.list.json`
carries a two-page example under `examples`.

`workspaces.lifecycle` (since 1.24.0, #135) announces the workspace reclaim
service. Each `workspace.list` row carries `lifecycle`, the service's last
verdict on that workspace, or `null` before the first sweep:
`{ measured_at_ms, last_activity_ms, idle, reclaimable, holds[], issues[],
dirty_files, unpushed_commits, behind_commits, size_bytes, pruned_bytes,
pruned_at_ms, noticed_at_ms }`. `holds` names what keeps the workspace from
being reclaimed: `not_ready`, `agent_working`, `terminal_open`, `dirty`,
`unpushed`, `plain_directory`, `issue_open`, `issues_unread`, `unknown` or
`unmeasured`. `pruned_bytes` stays 0 unless the bridge runs with
`BRIDGE_WORKSPACE_PRUNE` on. `issues` lists
`{ issue_id, number, title, status, state }` for each issue that links the
workspace. `workspace.reclaim` (`{ workspace_id }`, answered like
`workspace.delete`) removes the workspace. It refuses with `conflict` and a
sentence ("Build cannot reclaim quiet yet: it has uncommitted changes.") while
anything holds it, and with `busy` while another removal or the reclaim
service has the workspace. While a workspace is reserved (measured again
before its build output is moved, or before `workspace.reclaim` removes it),
every verb that would write inside it answers `busy`: `term.create`, the
`git.*` verbs that change a tree or its refs, `fs.write`, `fs.mkdir`,
`thread.attach`, `run.git_action`, `workspace.finish`, `workspace.delete`,
`workspace.rename`, `workspace.init_git` and the directory verbs. Reads are
answered as usual. Issue timelines gain three event kinds:
`workspace_idle` and `workspace_pruned`, both written by the new actor
`{ "kind": "build" }`, and `workspace_reclaimed`, written by whoever reclaimed
the workspace. Their payloads name the workspace (`workspace_id`,
`workspace_name`) and its size. None of the three wakes the issue's trackers.

`workspaces.reclaimBranches` (since 1.25.0, #167): once `workspace.reclaim`
has removed the workspace's checkouts, it deletes the local branch each one
carried, in the source repository it was cut from, under the rules
`branches.finishDelete` gives Done's post-removal measurement: never `main`,
`master`, a configured base or the branch a remote's `HEAD` names (each remote
is asked); never a branch checked out, rebased or bisected anywhere; never one
holding commits no remote has; and only at the commit those checks passed,
with the checkouts read again before and after. Nobody named these branches,
so a branch that has to stay never refuses the reclaim: the workspace goes and
the branch stays. The answer is `workspace.delete`'s plus `branches`, one entry
per repository a branch was taken from: `{ source_id, repository, branch,
outcome, reason? }`, where `outcome` is `deleted`, `kept` (with `reason`, the
sentence "Build cannot delete the branch <name>: …" that says why it stayed)
or `restore_failed` (a checkout moved onto the branch while it was deleted and
putting it back failed: the ref is gone, and `reason` names the commit, the
repository and the checkout). A branch that was already gone has no entry.
Each issue linking the workspace or the branch gets one event per entry,
written by whoever reclaimed and waking nobody, whose payload is the entry
plus `workspace_id`, `workspace_name` and `reclaimed: true`: `branch_deleted`
for `deleted` and `restore_failed`, and the new kind `branch_kept` for
`kept`. A bridge that does not announce the name leaves every branch where it
was, and answers without `branches`.

`settings.workspaceLifecycle` (since 1.25.0, #167) puts the reclaim service's
idle threshold and prune switch on `settings.*` as device settings, written to
`config.json`. `settings.get` (and every `settings.set` answer) carries
`workspace_idle_secs`, the threshold the service sweeps by now (24 h until the
device chooses), `workspace_prune`, whether it drops quiet workspaces' build
output now (off until chosen), and `workspace_pinned`, the names of those two
that an environment variable sets on this machine (`BRIDGE_WORKSPACE_IDLE_SECS`,
`BRIDGE_WORKSPACE_PRUNE`). A variable stays the override it was: the answer
reports its value, and a `settings.set` of a pinned field records the device's
choice for when the variable goes without changing what the service does.
`settings.set` takes `workspace_idle_secs` (whole seconds above 0, refused
`invalid_params` otherwise) and `workspace_prune` (a boolean); a change asks
for a sweep at once. `workspace_idle` events now carry `idle_after_secs`, the
threshold the workspace went quiet by. A bridge that does not announce the
name answers `settings.get` without the three fields and refuses the two params
as undeclared.

The names in the table below describe additions to existing verbs, so the
verb names alone cannot establish whether a bridge provides them. The SPA
gates features mapped by its capability adapter, including branch deletion;
other names announce support for clients that choose to consume them:

| Feature name | Shape or behavior announced | First available |
| --- | --- | --- |
| `messages.issueNotices` | Structured `issue_notice` on thread messages | 1.6.0 |
| `board.usageLimits` | `usage_limits` rows in `board.list` | 1.11.0 |
| `threads.newestDeltaPagination` | `thread.page` accepts `newest` with `after_sequence` to return the newest page of a delta | 1.12.0 |
| `issues.commentUserMentions` | `mentions_user` on issue comments | 1.13.0 |
| `issues.agentIdentities` | Durable `identities` map on issue views | 1.16.0 |
| `issues.attachmentChunks` | `issues.attachment` accepts `offset` and `length` for chunk reads | 1.19.0 |
| `params.strict` | A v1 verb refuses a top-level param its type does not declare (`invalid_params`, `unknown param: <name>`) | 1.24.0 |
| `workspaces.lifecycle` | `lifecycle` on `workspace.list` rows, and the `workspace.reclaim` verb | 1.24.0 |
| `branches.finishDelete` | `branch.finish` accepts `action: "delete"` to finish the workspace and delete its local branch | 1.24.0 |
| `changes.refusedKinds` | A `changes.subscribe` refused for an unknown kind names every such kind in `details.kinds` | 1.24.0 |
| `settings.workspaceLifecycle` | `workspace_idle_secs`, `workspace_prune` and `workspace_pinned` on `settings.*` | 1.25.0 |
| `workspaces.reclaimBranches` | `workspace.reclaim` deletes the workspace's local branches where safe, and `branch_kept` on issue timelines | 1.25.0 |
| `issues.listPaged` | `issues.list` accepts `limit` and `cursor` and answers `next_cursor` while more rows follow | 1.25.0 |
| `bodies.pages` | `fs.read`, `git.diff`, `git.show` and `git.changeset_diff` accept `range` and answer one page of whole lines with its `range` | 1.26.0 |
| `issues.createdUserMentions` | An agent's `create_issue` with `mention_user: true` marks the `created` event with optional `mentions_user: true` and watches the issue for the user | 1.27.0 |
| `board.conversationSessions` | A conversation's feed row (`board.list` items, `state` pushes) carries its own `session_started_ms` and `last_activity_ms`; the project conversation's row is how the inbox orders the project agent (#103) | 1.28.0 |
| `issues.unreadCounts` | A watched issue on `issues.list` and `issues.get` carries `unread_count`: the timeline entries after its `read_through` that are not the user's own and are news — comments, and `assigned`, `unassigned`, `moved`, `closed` and `reopened` events, never bookkeeping such as `created`, `tracked` or `linked` (#183) — the count its inbox row says. An unwatched issue carries none (#104) | 1.29.0 |

For a greeting at 1.22.0 or newer, the array is authoritative for the feature
gates implemented by the current SPA adapter: an absent name leaves its
corresponding capability flag off, even when the minor version would otherwise
suggest it. Announcing other feature or verb names does not create new SPA
gates beyond those mapped by the adapter. For older 1.x greetings
with no array, the v1 adapter keeps the historical feature mapping below. An
explicit older nested boolean wins over the version default when present.
Unknown names are ignored. A missing or malformed list on 1.22+ enables none
of the adapter's feature flags; a valid list on an older bridge also takes
precedence over fallback.
The older `messages.context` and `threads.postOperations` flags come from
`message_context.version == 1` and `thread_post_operations.version == 1`
(with a string `status_method`), respectively, rather than an inferred minor.
Those historical checks live beside the minor mapping in the adapter's one
legacy table; do not extend that table for new features.

| Feature name | Historical first minor |
| --- | --- |
| `changes.subscriptions` | 1.1.0 |
| `requests.priority` | 1.1.0 |
| `errors.codes` | 1.1.0 |
| `diffs.perFile` | 1.4.0 |
| `issues.context` | 1.5.0 |
| `issues.attachments` | 1.8.0 |
| `issues.watching` | 1.9.0 |
| `conversations.settings` | 1.10.0 |

When adding a verb, register it in its typed family or the small explicit
legacy list, add its method fixture, and let the greeting enumerate it. When
adding a cross-verb feature, add its name to the feature list and greeting
fixture. Update `API_VERSION` and `versions.json` once for the release.

SPA test (`spa/test/apiContract.test.js`): the v1 adapter parses every
fixture's `result` and every event example without throwing, and its
declared range admits `versions.json`'s `current`. The same fixture, two
consumers, the pattern `chat_operation_contract.json` already set; that file
folds into `fixtures/api/v1/thread.post.json`.

#### Step 2.4: structured errors

Additive in 1.1, so `error` stays a string. Beside it:

```json
{ "id": "r12", "ok": false, "error": "unknown method: changes.subscribe",
  "error_code": "unknown_method", "retryable": false,
  "details": { "method": "changes.subscribe" } }
```

`ApiError` in `api/mod.rs` is a closed enum: `unknown_method`,
`invalid_params`, `not_found`, `conflict` (a stale `expected_revision` or
`expected_choice_revision`; details carry the current value),
`unavailable` (isolation backend, harness not installed), `busy` (queue
full, `retryable: true`), `unsupported_version`, `internal`. Every `Err(String)`
in a handler is converted at the facade with a code; a handler that cannot
name one returns `internal`, and a test counts `internal` per method so the
list shrinks over time. In `2.0.0` `error` becomes the object and the string
form goes.

#### Step 2.5: SPA adapters

`spa/src/core/bridgeApi/` (new):

```
index.js      selectAdapter(greeting) → adapter | { unsupported: "bridge" | "app" }
semver.js     parse, compare, satisfies(version, range)  — no dependency
v1/index.js   { range: ">=1.0.0 <2.0.0", create(call, greeting) }
```

`create` returns `{ call, capabilities, events }` where `capabilities` is
derived from the greeting's announced names. For older bridges without the
array, the adapter uses the fixed historical minor mapping above and older
nested flags, never method-refusal probing. Surfaces and
repositories ask `capabilities`, never the version string. The adapter also
owns error normalisation: a reply with `error_code` becomes an `ApiError`
with that code; one without becomes `ApiError("unknown")` carrying the
string, which is how a v1.0 bridge's errors reach a view.

`greetBridge` (`changeEvents.js`) calls `selectAdapter` on every greeting
and installs the result on the session; a device switch or reconnect onto a
different bridge version re-selects. Two gates in `views/gate.js`:

- bridge major above every adapter's range → "This app is behind the bridge
  on `<device>`", with the served-version watcher's reload.
- bridge major below every adapter's range → "The bridge on `<device>` needs
  updating", with the install command.

The four cases, and what each end does:

| bridge | SPA | outcome |
| --- | --- | --- |
| 1.0 | 1.1-aware | adapter v1 with `changes.subscriptions: false`; legacy events, poll fallback, string errors |
| 1.1 | 1.0-only (stale tab) | bridge answers legacy by default; served-version watcher offers reload |
| 2.x | v1 adapter only | gate: update the app |
| 1.x | v2 adapter only | gate: update the bridge (does not arise while the bridge serves N-1) |

#### Step 2.6: `from_agent` on a thread message

A thread message carries `from_agent: {"id": "<agent id>"}` when an agent
wrote it and not the human. `role` stays `user`: it says which side of the
conversation a message is on — inbound, where the human's words land — not
who typed it, so `MessageRole` is unchanged and a client that has never
heard of `from_agent` renders the message exactly as it always did. The
field is omitted entirely on everything the human said, which makes it a
minor, additive change (step 2.1).

The router sets it on the instruction `branch.dispatch` lands in the new
agent's conversation, and the message travels into the native delivery
payload with it, so the receiving harness reads who is handing the work
over. A message carrying `from_agent` never counts as the human
interacting with the work: they did not send it, and the inbox must not
say they did.

`from_agent` widened, additively, to say where the sender was speaking from:

```
from_agent: {
  id: string,
  owner?: { kind: "workspace" | "project", id: string, name: string },
  topic?: string,
  name?: string
}
```

`owner.id` is the workspace id when `kind` is `workspace` and the project id
when it is `project`; `owner.name` is that workspace's or project's display
name. `topic` is what the sending agent last called its own conversation
(`set_topic`), and it may be the empty string — a conversation that was read and
has not named itself yet. `name` is what the agent is CALLED (**1.7.0**; see
Agent names below), absent until somebody names it. All are stamped by the
bridge at POST time, from what it knew then: a workspace can be renamed, a topic
changes with the work, and what the message says is what was true when it was
sent.

### Agent roles and capability (1.7.0)

Every create path took free-text `harness`, `model` and `effort`, with no list
of what was accepted — so an agent guessed, and a wrong guess was a refused
call or a silently defaulted agent.

**The device declares which models are for which roles.** `settings.role_models`
is a list, and the ORDER is the user's preference: when two models can both
review, the one nearer the top reviews.

```json
"role_models": [
  { "model": "claude-fable-5-1", "roles": ["planner", "reviewer"], "capability": "generalist" },
  { "model": "claude-opus-5", "roles": ["planner", "reviewer", "implementer"], "capability": "scoped" },
  { "provider": "codex", "model": "gpt-5", "roles": ["implementer", "executor"], "capability": "step_by_step" }
]
```

Roles are `planner`, `implementer`, `reviewer`, `executor`. `provider` absent
means the device's `default_harness`. A model declared for no roles stays in
the list and is never chosen, which is a legible thing to want.

**Capability is an output, not an input.** It says how much direction that
model needs from whoever hands it work — `generalist`, `scoped`,
`step_by_step` — and the create path answers it back so the agent writing the
brief knows whether to write a goal, a scope or a list of steps.

**Asking.** `agent.add`, `add_workspace_agent` and `issues.assign`'s creating
kinds take `role`, and optionally `capability` when the caller needs a
particular kind. The answer carries `capability` and `direction`, the latter
being the instruction in words. An explicit `harness`/`model`/`effort` is laid
over the role's answer **field by field**, so naming an effort does not discard
the model the user chose.

**Effort is never configured.** Which model fills a role is the user's standing
decision; how hard it thinks about one piece of work is the creating agent's,
and it passes `effort` itself.

**Nothing gates the model id.** A model outside this bridge's catalog is passed
through with only a shape check, so a model newer than the bridge is usable
(2026-09-20: "Don't gate"). The **harness** is a closed set — Build can
only run what it implements — and an **effort** must be one that harness
accepts; both are refused by name with the accepted values in the sentence.

**`list_harnesses`** (MCP, both working surfaces) is the lookup: every harness
with its models and efforts, **whether its binary is on this machine's PATH**,
the roles and capabilities there are, the declared list, and which model
answers each role right now. `models.list` carries `role_models` and each
provider's `binary`/`installed` too, so a client reading the catalog needs no
second call.

### Agent names (1.7.0)

Every agent was labelled by ordinal — "Agent 1", "Agent 2" — which says where it
sits in a rail and nothing about what it is. An agent now carries a **`name`**:
one or two meaningful words ("Tracker", "Rail scroll", "Transport"), on the
agent digest (`agent.add`, `agent.list`, `workspace.agents`,
`list_workspace_agents`), on `from_agent`, and on `issue_notice.actor`. It is
absent until somebody sets one, and a client falls back to the ordinal for
exactly that long — which is also what an older client does with it.

A name is not a topic. A topic is a subject line that moves with the work; a
name is who the agent is, and it is unique among the agents of one conversation.

**Set three ways.** `agent.add` and `add_workspace_agent` take an optional
`name`. `issues.assign` with `{kind:"new_agent"}` or `{kind:"new_workspace"}`
takes an optional **`agent_name`** — spelled differently because `name` on
`new_workspace` is already the workspace's, and one key meaning two things is
how a caller names the wrong one. And the agent names itself with the
**`set_name`** MCP tool, which the project agent is not offered: it is named by
its project.

**Validation**, in the same words wherever it is refused, because an agent reads
the refusal and repeats it to the user: 2 to 24 characters, at most three words,
trimmed with inner whitespace collapsed, and not a name another agent on that
conversation already has — compared case- and spacing-insensitively, so "Rail
scroll" and "rail  scroll" are one name. An agent re-stating its own name is not
a clash.

**The ask.** The first message the USER sends to an agent that has no name
carries one instruction with it: name yourself with `set_name` before answering.
Once per agent and never again — an agent that was asked and did not do it has
decided, and asking on every message would be nagging in the user's own voice. An
agent woken by another agent's hand-off is not asked: it is being given work, not
greeted, and a name the user will read should be chosen while the user is there.

Every message carrying `from_agent` is one an agent sent on purpose — a router
dispatch or a `message_agent`. The bridge writes none of its own: it used to
forward a target agent's terminal report into the conversation of the agent that
asked, and that hand-off is gone (agent surfaces spec, revision 2026-09-18b), so
a reply between agents is always a send. Nothing on the wire changed with it;
`requested_by` was always internal, on no wire shape.

Both are omitted where the bridge cannot name them, and on every record written
before they existed, so a client that reads only `id` is unaffected. They are
what lets a client draw `{workspace|project} > {conversation}` over an inbound
agent message and link both halves without a second read. `AgentIdentity` is
also the conversation's own `agent` on `thread.page`, which is never stamped and
stays `{id}`.

#### Step 2.6b: `sent_to` on a thread message

The other half of `from_agent`. A message carries

```
sent_to?: {
  id: string,
  owner?: { kind: "workspace" | "project", id: string, name: string },
  topic?: string
}
```

when it is the record, in the SENDER's own conversation, of a message that agent
sent to another agent's. Same shape as `from_agent` and stamped the same way, at
post time, naming the RECIPIENT. So both ends of a hand-off are on a page: the
recipient's conversation holds the words wearing the sender, and the sender's
holds them wearing the recipient, and either can be drawn as
`{workspace|project} > {conversation}` and linked.

`role` is `agent`: the agent wrote the words. `from_agent` is absent — nobody
handed this conversation anything. `still_working` is true, because calling a
tool is not handing the turn back.

What it counts as:

- a MESSAGE — it spends a page's budget (`message = 1` in the store) and a run
  of activity is cut around it, so a page shows it and it opens a new activity
  container;
- not a hand-off (`handoff = 0`), and not the conversation's own two parties
  speaking: it does not draw the line a dismissal is judged against, so a row
  the human cleared stays cleared;
- not attention: nothing unread, no badge, nothing calling the human. The agent
  wrote to another agent, not to them.

The field is omitted on every message that went nowhere, which is every message
written before agents could write to each other, so a client that has never
heard of it is unaffected. In a cold catch-up packet the same fact is one
trailer on the message's own line — `[sent to agent <id>]`, beside the
`[from agent <id>]` the other direction carries.

#### Step 2.7: `project_agent` on `settings.*`

`settings.get` carries `project_agent: { provider?, model?, effort? }` — what
this device says a project's agent starts on — beside `default_harness`, which
is what every other agent falls back to. Each word may be absent, and each
absent one names a default that already stands: no provider is
`default_harness`, no model or effort is that harness's own. A device that has
chosen nothing answers `{}`.

`settings.set` takes the same object and is partial the way the set around it
is: a word the object leaves out stands, a `null` word clears that one, and a
`null` object clears all three. The set answers with `settings.get`, so a panel
repaints from what the device now holds rather than from what it attempted. The
choice is written to `config.json` beside `default_harness` and read back at
boot; a value this bridge cannot read is logged and left absent.

Additive, so a minor: a client that has never heard of `project_agent` sends
none and reads past it, and the bridge answers exactly as it did.

#### Step 2.8: `viewing_context` on a reviewer message

A message the human sends may carry `viewing_context: { version: 1, items: [] }`
— what they were looking at when they wrote it. It rides every reviewer send
(`thread.post`, `run.message`, `plan.message`, `issue.send_notes`), is stored on
the message, and reaches the harness on the native delivery payload. The bridge
offers it on the greeting as `message_context: { version: 1 }`; a client that is
not offered it sends none.

Each item is tagged by `kind`:

- `file { path }` — a file open on screen, scope-relative.
- `diff { path, mode }` — a diff on screen, `mode` one of `uncommitted`, `all`.
- `commit { sha }` — a commit on screen, as a full 40- or 64-character id.
- `selection { path, text, line_start?, line_end?, side?, unsaved?, truncated? }`
  — a passage they had selected.
- `workspace { workspace_id, name }` — the workspace they were standing in. The
  project's conversation is reachable from every workspace's rail, so a message
  sent from one leads with this item, and the delivery envelope says in prose
  which workspace the user was in.
- `issue { issue_id, number, title }` — the issue they had open on the board
  (**1.5.0**). The delivery envelope names it in prose too, and tells the agent
  to read it with `get_issue` before answering about it. It carries no body on
  purpose: an issue moves on after the message is sent, and a copy frozen into
  the context would go stale while reading as current. Being pointed at an
  issue is not being handed one — the sentence differs from the `from_issue`
  hand-off's, which is the agent's work rather than the user's screen.

A path must be scope-relative and at most 4 KiB; excerpts total at most 32 KiB
across a context; a context carries at most 100 items and at least one; a
workspace's id and name, and an issue's id and title, are each at most 512
bytes and none may be empty.
Neither the context nor an item denies an unknown field — a newer SPA must be
able to talk to an older bridge — so `version` stays 1 while kinds are added,
and an item this bridge has never heard of is refused by kind alone.

## Verification

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
  --check`; `npm run lint && npm test` in `spa/`.
- Bus: with the paused-clock helper in `changes.rs`, a realtime and a 30 s
  batch subscription over the same entity receive one item at 250 ms and one
  at 30 s respectively from a burst of a hundred notes; an `off` subscription
  receives nothing and then one item on the upsert back to batch; a
  foreground flush precedes a background flush due in the same turn.
- Watcher: a tempdir repo; a write under `src/` yields a `files` item with
  that path and a `git` item; a write under `.git/objects/` yields nothing;
  an ignored path yields `git` only; `start` against a path over the inotify
  limit answers `polled` and the TTL path still notes `git`.
- Dispatch: eight background frames held on an `OffLockGate` while a
  foreground `board.list` completes under 200 ms.
- Fixtures: `bridge/tests/api_contract.rs` and `spa/test/apiContract.test.js`
  green against the same `fixtures/api/v1/`; CI fails if either side adds a
  method without a fixture.
- Adapters: `selectAdapter` tests for each row of the four-case table and
  for a greeting with no `api_version`.
- Manual: two workspaces, an agent writing in the background one. The
  focused workspace's git pane updates within a second of a write; the
  background row's stat updates within 30 s; `bridge.stats` shows the
  background queue never starving the foreground one.
- Manual, the cache-first client's three: (1) a reload paints the full app
  from cache with no gate frame; (2) switching workspaces shows the agent
  bubbles and the thread on the first frame; (3) an agent writing in a
  background workspace moves that row's git surfaces within 30 s and the
  active workspace's within a second.
- `grep -rn "intervalMs\|setInterval" spa/src` names only the presence
  poll, the served-version check, the gate's own clock
  (`views/gate.js` `watchOnGateCadence`), the cosmetic clocks, and the
  `RETIRED_OPTIONS` constant at `core/changeEvents.js` — the list of the
  poll options `watchChanges` now refuses, which is a name in a string and
  not a timer. The gate's clock reads the account's REST device list, never
  a bridge: it runs while the gate is holding the page, which is when there
  is no machine to ask anything of.

## Out of scope

- Multi-device aggregation; scopes are device-local as the board is.
- Relay and skriftapp changes. Everything here is inside the E2EE session.
- The `2.0.0` error shape and retiring `plan.*`; noted so that they are the
  first entries on the v2 list, not done here.
- Diff-level deltas on the wire. `git.diff` with `content_key` already
  fetches per file; the event only says which files.

## Rollout

1. Step 2.0 on its own commit to `main` before the alpha cut: `api_version`
   in `session.hello` and `ping`, `client` recorded, SPA records the
   greeting's version. The alpha bridge is `1.0.0`.
2. Part 1 and the rest of Part 2 on `build/bridge-wire-interface`, bridge
   first (steps 1.2 to 1.5, 2.2 to 2.4), each step green with the existing
   SPA in legacy mode, then the SPA (1.6, 2.5). `API_VERSION` becomes
   `1.1.0` on the commit that ships `changes.subscribe`.
3. Merge to `main`, deploy the SPA, roll the local bridge. A user whose
   bridge is still `1.0.0` sees no change until they update; a user whose
   tab is stale gets the reload offer. Never deploy from this branch.
