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
- **Additive within a major.** Unknown fields are ignored by both ends;
  unknown methods and unknown event types are errors and no-ops respectively,
  never crashes.
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

`core/changeEvents.js` grows a subscription manager beside the watcher set.
A surface registers what it wants, not what to run:

```js
watchChanges({
  entity: () => [row.run_id, row.worktree_id],
  kinds: ["state", "thread", "git", "files"],
  mode: "realtime",           // mounted surfaces
  onChanges: (items) => …,    // key comparison, then the existing refresh
});
```

The manager keeps desired subscriptions as a map, diffs against what the
session has, and issues `changes.subscribe` / `changes.unsubscribe` on
mount, unmount, greet, and reconnect (`greetBridge` already re-greets on
every live session; it now also replays the desired map). The poll callback
survives as the 60 s safety poll for mounted surfaces and as the fallback
for a bridge whose greeting lacks `changes.subscriptions`.

Three tiers, and the whole of the client's policy:

| tier | who | subscription |
| --- | --- | --- |
| focus | the mounted work surface and the rail | `entity` scope, all kinds, `realtime`, `foreground` |
| board | the feed | `board` scope, `state`, `realtime`, `foreground` |
| background | everything in the feed's active set | `all` scope, `state`+`thread`+`git` at `batch_ms: 30000`; `all` scope, `files` at `batch_ms: 180000`; both `background` |

`core/cacheSync.js` stops looping. It becomes the background tier's
`onChanges` handler: for each item, compare against the IndexedDB record and
pull only on a miss, at `priority: "background"` through `readRequests`,
which now also stamps the request envelope.

- `state`: board `revision` differs → `board.list`; entity state differs →
  `run.get` / `issue.get` / `branch.get`.
- `thread`: `last_sequence` above the cached window's `deliveredSequence` →
  `thread.activity` from the cached sequence, write-through.
- `git`: `status_key` differs from the cached `status` record → `git.status`
  with `if_status_key` (still cheap on a race), then `git.log`.
- `files`: delete the cached `tree` record for each path's parent chain and
  the `filediff` record for the path; refetch only if a surface has it open.
  `truncated` deletes every `tree` record for the entity.

The safety sweep for the background tier is one full `cacheSync` pass every
10 minutes and on `visibilitychange`, replacing the 60 s loop.

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
- The bridge serves the current major and the previous major until the
  later of 90 days after the new major ships or `bridge.stats` showing no
  session on the old one for 30 days.
- `PROTOCOL_VERSION` (envelope), the MCP protocol date, and the Cargo
  version stay separate. They version different things.

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

Requests derive `Deserialize` without `deny_unknown_fields` (forward
compatibility: a newer SPA may send a field this bridge predates). Responses
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
derived from the greeting and the minor version, never from probing:
`changes.subscriptions`, `requests.priority`, `errors.codes`. Surfaces and
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
  background row's stat updates within 30 s; the background files tree
  refreshes within 3 minutes; `bridge.stats` shows the background queue
  never starving the foreground one.

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
