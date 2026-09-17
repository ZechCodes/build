# Worktree Surfaces — Binding Technical Spec

**Status:** Binding. Implementers follow this exactly; names and shapes below are the
contract between four sequential implementation layers. Where this spec pins a name,
use that name. Where behavior is unspecified, match the existing code's conventions
(fail fast, pure domain core, TDD, escape all untrusted display text).

**Feature summary.** Every worktree-backed surface (task, external worktree,
and a NEW primary-checkout "main" surface per project) gets a unified tab shell:
its existing work tab(s), a **Files** tab (browse + per-type preview), an
**Agent** tab (task surfaces only — the live agent PTY), and any number of
user **terminal tabs** with a `+` to create more. Terminals become **keyed**
(`term_id` on every RPC and push) and scoped to a worktree root. The backtick
overlay drawer is removed entirely; all terminal traffic rides ONE dedicated
E2EE terminal socket per browser tab, demuxed by `term_id`. `task.list` gains a
cached primary-changes summary; the sidebar and project page gain a "main"
entry. Design rule #3 holds: the terminal is the basement — always accessible
(one tap from any worktree surface), never the default tab.

## Conversation surface amendment (July 29, 2026)

This amendment supersedes the tab-order and embedded-review-conversation language
below. Managed run surfaces use `Conversation | Stages? | Changes | Files | Agent |
terminals`; external worktrees use `Conversation | Changes | Files | Agent |
terminals`. Conversation is first and is the default route. Diff review retains
selection, whole-file, and whole-change-set comments, but submitted comments are
durable user messages shown only in Conversation. An external worktree creates its
durable conversation on first agent-directed action through the existing adoption
flow. Agent messages and status events may carry bridge-validated typed references
to worktree files, plan stages, and runs; the SPA renders references as scoped
navigation controls, never arbitrary links.

## Primary-checkout retirement amendment (September 17, 2026)

This amendment supersedes the primary-checkout "main" surface below. A project's
own checkout is what workspaces are cut from, never a place to work: users create
a workspace and Finish upstreams it. The bridge no longer adopts the repo root as
a super-worktree, and everything that surface needed is gone with it —
`run.adopt {primary: true}` is refused in words that name workspaces, the
`primary` field has left every feed, run and pending row on the wire, and
`task.list`/`board.list` no longer carry a `primary_changes` summary or its
`PRIMARY_SUMMARY_TTL` cache.

The project's repository is still read: project-scoped `fs.*`, `project.diff`,
`git.branches` and terminals resolve to it exactly as before, and the branch
listing still names it as a holder — under the wire kind `project_repository`,
where it once said `primary_checkout`. A store written before this holds runs
adopted on the repo root and `row:<project_id>:primary` attention keys; both load,
the stale key is ignored and pruned, and no discard verb may remove a checkout
whose canonical path is the project's repository.

In the SPA the "main" entry is replaced by the project's own page
(`planning/v2/workspaces.md`, "The project page"), and the client's half of the
retirement went with it: `primaryAdoptScope` / `createPrimaryAdoptingCall`, the
branch picker's `primary_checkout` row, the feed's `primaryChanges` collection
and `primaryRunIdFor`, the `primary` field on inbox entries and rail entities,
the pending-row arm that matched a project's own card, and the
`{project_id, primary: true}` dismissal. What a checkout IS is read off the
row's shape instead of the retired flag: a branch row that names neither a run
nor a worktree is the project's own directory, and `{project_id}` is the scope
it answers under — which is what a plain folder (a project with no git in it)
has always been.

---

## 0. Global decisions (read first)

1. **Scopes are resolved server-side only.** A terminal or fs scope is one of
   `{task_id}`, `{project_id, worktree_id}` (external), or `{project_id}`
   (primary checkout). The bridge maps ids to roots via its own records
   (`ActiveTask.worktree.path`, the external-worktree scan, `Project.repo_path`).
   Client-supplied filesystem paths are NEVER scope roots. `fs.list` (the host
   directory browser for repo picking) is untouched and remains the only
   unfenced directory surface — it lists directories only and reads no files.
2. **`term_id` scheme.** User terminals: `term-<n>` where `<n>` comes from a
   monotonically increasing in-memory counter (`next_term`, starts at 1, never
   reused within a daemon lifetime). Agent terminals: the reserved id
   `agent:<task_id>` (e.g. `agent:task-3`). `term.create` never mints an
   `agent:` id; `term.attach`/`term.close` reject `agent:` ids (the agent tab
   uses `agent.attach`, and agent PTY lifetime belongs to the orchestrator);
   `term.input`/`term.resize` accept both.
3. **The legacy un-keyed `term.*` path is dropped.** `term.attach` without a
   `term_id`, and `term.input`/`term.resize` without one, return an error. The
   single global `AppState.term: Option<TermSession>` (daemon-cwd bash) is
   deleted. The SPA is the only client and ships in the same image; the Node
   harnesses in `web/` that used the old path are updated in Layer 4.
4. **One pump task per terminal.** Each live terminal (user or agent) gets its
   own `tokio::spawn`ed pump. Justification: the existing single-terminal pump
   (`select!` over `rx.recv()` + a 10 ms flush tick) is proven; iterating one
   task over N broadcast receivers would need a merged-stream abstraction, a
   shared flush cadence, and manual removal bookkeeping — for zero measurable
   win at our scale (hard cap of 16 user terminals + one agent screen per task,
   each pump a few hundred bytes of state). Per-terminal tasks keep flush
   timing independent (one flooding terminal never delays another's flush),
   terminate naturally on PTY EOF, and each pump touches shared state only
   under the existing short-held `Mutex` — exactly like today.
5. **Terminal cap.** At most **16 user terminals** daemon-wide (all scopes
   combined). `term.create` at the cap fails with
   `"terminal limit reached (16 open terminals) — close one first"`. Agent
   screens don't count (at most one per task, bounded by tasks). Plus a reaper
   (§2.7) so orphaned terminals never accumulate even below the cap.
6. **Terminals persist across page reloads, not daemon restarts.** The PTY and
   its vt100 screen model live in the daemon; a reloaded SPA calls `term.list`
   and re-attaches (snapshot + cursor). When the daemon restarts, child PTYs
   die with it; `term.list` returns empty and the SPA shows no terminal tabs.
   No terminal state is persisted to the store.
7. **All `term.*` and `agent.attach` traffic rides the dedicated terminal
   socket** (the SPA's one-per-browser-tab `TerminalSocket`, §6). `fs.tree`,
   `fs.read`, `project.diff`, and everything else ride the app RPC session as
   usual. Rationale: pushes go to the *attached sender's* session, so attach
   must happen on the socket that will render the bytes; keeping create/list/
   close on the same socket gives the manager a single serialized owner of
   terminal lifecycle and keeps PTY floods off the app session entirely (no
   head-of-line blocking of RPCs).
8. **Tab-shell interpretation for tasks.** The task view keeps BOTH its Plan
   and Diff tabs with their existing content untouched. "Primary work tab"
   means the *default-active* tab: `plan` while state ∈ {created, planning,
   plan_review}, `diff` otherwise; an explicit tab in the route always wins.
   New tabs are appended after Diff: `Files`, `Agent`, then one tab per open
   user terminal, then `+`.
9. **Backward compat is non-negotiable.** Legacy single-plan tasks, quick
   tasks, multi-stage flows, adoption flows, and the existing qa.mjs checks
   keep passing unchanged (qa.mjs is *extended*, §8; existing checks are not
   edited).
10. **HTML/SVG preview is sandboxed** (§7.4): HTML in `<iframe sandbox="">`
    (no scripts, no same-origin), SVG only ever via `<img src="data:...">`,
    never inlined into the DOM. Every file name/path rendered goes through
    `esc()`.

---

## 1. Scope model and server-side resolution (bridge, shared)

New types in `bridge/src/app.rs` (they touch `AppState` lookups, so they are
app-layer, not `task.rs` domain):

```rust
/// A worktree-backed surface a terminal or fs call is scoped to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TermScope {
    Task { task_id: String },
    ExternalWorktree { project_id: String, worktree_id: String },
    Primary { project_id: String },
}
```

**Wire form:** scope fields ride inline in `params` (no nested object):

| params present                | scope                                        |
|-------------------------------|----------------------------------------------|
| `task_id`                     | `Task` (wins even if `project_id` also sent) |
| `project_id` + `worktree_id`  | `ExternalWorktree`                           |
| `project_id` only             | `Primary`                                    |
| none                          | error `"missing scope: task_id or project_id required"` |

```rust
impl TermScope {
    fn parse(params: &Value) -> Result<TermScope, String>;
    /// Resolve to the scope's canonical root directory, server-side only.
    fn resolve_root(&self, state: &mut AppState) -> Result<std::path::PathBuf, String>;
}
```

`resolve_root` resolution table (each arm errors with the message shown when
the id is unknown):

| scope              | root                                                            | error                       |
|--------------------|-----------------------------------------------------------------|-----------------------------|
| `Task`             | `state.tasks[task_id].worktree.path` (must exist on disk)       | `"unknown task_id"` / `"worktree no longer exists"` |
| `ExternalWorktree` | `state.resolve_external_worktree(project_id, worktree_id)?.path` (already canonical, from the cached ≤10 s scan) | its existing errors (`"unknown worktree_id: …"`) |
| `Primary`          | the registered `Project.repo_path`                              | `"unknown project_id"`      |

`resolve_root` takes `&mut AppState` because the external-worktree arm may
refresh the scan cache (existing behavior). It never accepts a raw path and
never canonicalizes client input.

---

## 2. Keyed terminals (bridge, Layer 1)

### 2.1 State reshape

In `bridge/src/app.rs`:

- Extract the screen-model half of today's `TermSession` into a reusable
  struct (used by both user terminals and agent screens):

```rust
/// Authoritative server-side screen: vt100 model + attach list + coalescing
/// buffer + the monotonic byte cursor. Snapshot resync, not byte replay.
struct TermScreen {
    parser: vt100::Parser,        // vt100::Parser::new(rows, cols, 2000)
    attached: Vec<SessionSender>,
    pending: Vec<u8>,
    total: u64,
    cols: u16,
    rows: u16,
}
```

  with `fn snapshot(&self) -> String` (b64 of `contents_formatted()`, as today)
  and `fn set_size(&mut self, cols: u16, rows: u16)`.

- `TermSession` becomes:

```rust
struct TermSession {
    term_id: String,
    scope: TermScope,
    scope_root: std::path::PathBuf,   // resolved at create time
    created_at: String,               // RFC 3339, like task_created_at
    session: PtySession,
    screen: TermScreen,
}
```

- `AppState.term: Option<TermSession>` is **replaced** by:

```rust
terms: HashMap<String, TermSession>,      // user terminals, keyed by term_id
agent_screens: HashMap<String, AgentScreen>, // §3, keyed by task_id
next_term: u64,                            // term-<n> mint counter, starts 1
```

- Constants: keep `TERM_FLUSH_MS = 10` and `TERM_SNAPSHOT_THRESHOLD = 128 * 1024`.
  Add `MAX_USER_TERMINALS: usize = 16`.

- `TermSession::spawn(cols, rows)` gains the cwd:
  `fn spawn(term_id, scope, scope_root, cols, rows) -> Result<(TermSession, broadcast::Receiver<Vec<u8>>), String>`
  — same `bash --norc -i`, `TERM=xterm-256color`, `PS1="build$ "` spec, but
  `PtySession::spawn(&spec, Some(scope_root.clone()), size)`.

### 2.2 RPC surface (exact JSON)

All of these ride the terminal socket (§0.7) but the bridge does not enforce
which session calls them. Dispatch placement: `term.create` and `term.attach`
and `agent.attach` are handled in `dispatch_frame` (they need the `Arc` to
spawn pumps and/or the `SessionSender`); `term.list`, `term.close`,
`term.input`, `term.resize` go through `AppState::dispatch`.

**`term.create`** `{ task_id? | project_id [+ worktree_id], cols, rows }` →

```json
{ "term_id": "term-3", "cols": 120, "rows": 30 }
```

Parses the scope, resolves the root (§1), enforces the cap (§0.5), mints
`term-<next_term>`, spawns the shell in the root, inserts into `terms`, and
**immediately** starts its pump (§2.4) — the screen model accumulates even
before the first attach. `cols`/`rows` default 80/24 when absent (match
`term_attach` today).

**`term.list`** `{ task_id? | project_id [+ worktree_id] }` →

```json
{ "terminals": [
    { "term_id": "term-3", "cols": 120, "rows": 30, "created_at": "2026-07-11T…Z" }
] }
```

Only user terminals whose `scope` equals the parsed scope, ordered by numeric
id suffix ascending. Never includes agent ids. Unknown-scope ids still error
(the SPA treats an error as "no terminals").

**`term.close`** `{ term_id }` → `{ "ok": true }`

Rejects `agent:` ids with `"cannot close an agent terminal"`. Removes the
entry from `terms`, calls `PtySession::kill_and_reap()` (kill **and** wait —
the existing zombie-prevention contract), then pushes to every attached sender:

```json
{ "type": "term.closed", "term_id": "term-3", "reason": "closed" }
```

Unknown `term_id` → `"unknown term_id"`.

**`term.attach`** `{ term_id, cols, rows }` →

```json
{ "term_id": "term-3", "snapshot": "<b64>", "cursor": 12345, "cols": 120, "rows": 30 }
```

Same semantics as today's attach, per terminal: under one lock — resize PTY +
parser if the caller's grid differs, drop any prior sender with the same
session id, push the caller's sender, snapshot + cursor. Rejects `agent:` ids
with `"use agent.attach"`. It no longer creates anything (`"unknown term_id"`
when absent) — creation is `term.create`'s job.

**`term.input`** `{ term_id, data }` (data = b64 keystrokes) → `{ "ok": true }`

Routes to `terms[term_id].session.write_input`. For `agent:<task_id>`: routes
to the task's live session via a new
`ActiveTask::write_input_strict(&self, bytes) -> Result<(), String>` that
errors `"no active agent session"` when `session` is `None` (the existing
silent `write_input` stays for the legacy callers). Unknown id → `"unknown term_id"`.

**`term.resize`** `{ term_id, cols, rows }` → `{ "ok": true, "live": true }`

User terminals: resize PTY + `screen.set_size` (as today). Agent ids: when a
session is live, resize the agent PTY + agent screen and return `live: true`;
when none is live, do nothing and return `{ "ok": true, "live": false }` (the
retained last screen must not be garbled by a dead resize — and attach must
never error the whole view, §3).

**`term.ack`** `{ term_id, cursor }` → `{ "ok": true }`

Terminal flow control, handled in `dispatch_frame` (it needs the caller's
`SessionSender` — an ack speaks for one client's receive queue). The client
reports the cursor it has applied; the bridge stores it against that client's
attachment. A client more than `TERM_UNACKED_BUDGET_BYTES` past its last ack is
paused: it receives no `term.output` and no flood-collapse `term.reset` until it
has acked everything it was actually sent (a paused client is fed nothing, so
the live cursor runs away from it without bound — its own last sent frame is
the most it can ever ack, and acking that means its queue has drained). It then
gets exactly one `term.reset` snapshot at the live cursor (it missed frames, so
raw bytes would no longer be contiguous), which rebaselines its acked cursor
the way the attach snapshot does, and resumes. A client that has never acked is
exempt — an older SPA sends none, and it keeps the unthrottled behaviour.
Unknown id → `"unknown term_id"`.

### 2.3 Push frames (all carry `term_id` now)

```json
{ "type": "term.output", "term_id": "term-3", "data": "<b64 bytes>",   "cursor": 12345 }
{ "type": "term.reset",  "term_id": "term-3", "data": "<b64 snapshot>", "cursor": 12345 }
{ "type": "term.closed", "term_id": "term-3", "reason": "closed" }
```

`reason` ∈ `"closed"` (explicit `term.close`), `"exited"` (the shell process
ended), `"reaped"` (scope vanished, §2.7), `"agent_session_ended"` (agent
screens only, §3.4). Clients dedupe output/reset on `cursor > lastCursor` per
`term_id`, exactly as today.

### 2.4 The per-terminal pump

`spawn_term_pump(state: Arc<Mutex<AppState>>, term_id: String, rx: broadcast::Receiver<Vec<u8>>)`
— today's pump body, with the `s.term.as_mut()` lookups replaced by
`s.terms.get_mut(&term_id)` and `term_id` added to every push payload. Two
behavioral changes:

- If the lookup returns `None` (closed under it), the pump task returns.
- On `RecvError::Closed` (PTY EOF — the shell exited): lock, remove the entry
  from `terms`, `kill_and_reap()` it (reap the exit status), push
  `term.closed` with `reason: "exited"` to its attached senders, return.

`Lagged` is `continue`, as today.

### 2.5 Session-drop cleanup

`AppState::drop_session(session_id)` now retains across **every** `terms`
entry's `screen.attached` and every `agent_screens` entry's `attached` (was:
the one global terminal).

### 2.6 Lifecycle rules (normative)

1. **Persist across reloads:** nothing closes a user terminal on client
   disconnect — `drop_session` only detaches senders.
2. **Explicit close:** the tab's `×` calls `term.close` (§2.2).
3. **Auto-close (reaper §2.7):** a user terminal closes when its scope no
   longer resolves:
   - `Task` scope: the task id is no longer in `state.tasks` (deleted,
     released) **or** `scope_root` no longer exists on disk (worktree pruned by
     abandon / merge-with-prune). A merged task with `cleanup=keep` keeps its
     worktree → its terminals stay.
   - `ExternalWorktree` scope: `scope_root` no longer exists on disk. (If the
     worktree was *adopted*, the path still exists — the terminal survives;
     its shell cwd is still valid.)
   - `Primary` scope: the project is no longer registered, or `scope_root`
     no longer exists.
4. **Kill guarantees:** every close path (`term.close`, pump-EOF, reaper) goes
   through `PtySession::kill_and_reap()`. The blocking reader thread exits on
   the EOF that follows the kill. No zombies, no leaked reader threads.
5. **Cap:** §0.5.

### 2.7 The reaper

```rust
/// Close every user terminal whose scope no longer resolves (§2.6.3), and drop
/// every agent screen whose task record is gone. Returns the closed term_ids.
fn reap_orphaned_terminals(&mut self) -> Vec<String>
```

Pushes `term.closed { reason: "reaped" }` to each closed terminal's attached
senders. Called from two places:

- the tail of `finish_mutation` (prompt closure right after abandon / delete /
  merge-prune / release), and
- `AppState::spawn_terminal_reaper(state, interval)` — a `tokio::spawn` loop
  (started in `main.rs`/`service.rs` beside `spawn_idle_monitor`) with
  `interval = Duration::from_secs(30)`, catching out-of-band disappearance
  (user `rm -rf`s an external worktree).

The check is cheap: ≤ 16 entries, a `tasks` lookup and/or one `Path::exists`.

---

## 3. Agent tab plumbing (bridge, Layer 1)

### 3.1 State

```rust
/// The retained screen of a task's agent PTY stream. Created on first
/// agent.attach, retained until the task record is removed (reaper), so the
/// tab can show the last screen between sessions.
struct AgentScreen {
    screen: TermScreen,
    /// The ActiveTask session generation this screen's pump is consuming.
    /// 0 = no pump has ever run.
    pumped_generation: u64,
    /// Whether a live pump is currently feeding this screen.
    live: bool,
}
```

`ActiveTask` gains `pub session_generation: u64` (starts 0), incremented in
`Orchestrator::spawn_session` right where `active.session = Some(session)` is
set, and a getter pair:

```rust
/// The live session's generation + a fresh subscription, if one is warm.
pub fn subscribe_with_generation(&self) -> Option<(u64, broadcast::Receiver<Vec<u8>>)>
```

(the existing `subscribe()` stays for the idle monitor). `reattach` and every
manual `ActiveTask` construction initialize `session_generation: 0`.

### 3.2 `agent.attach`

`{ task_id, cols?, rows? }` → handled in `dispatch_frame` (needs Arc + sender):

```json
{
  "term_id": "agent:task-3",
  "live": true,
  "provider": "claude",
  "snapshot": "<b64>",
  "cursor": 4096,
  "cols": 120,
  "rows": 40
}
```

Behavior, all under one lock:

1. `task_id` must be in `state.tasks` → else `"unknown task_id"` (a deleted
   task's agent tab errors; the SPA falls back to its quiet state).
2. Get-or-create `agent_screens[task_id]` with grid = the caller's
   `cols`/`rows`, defaulting to the orchestrator's PTY size **40 rows × 120
   cols** when absent.
3. If a session is live (`subscribe_with_generation()` is `Some`) and its
   generation != `pumped_generation`: start a new agent pump (§3.3) for it,
   record the generation, set `live = true`.
4. If a session is live and the caller's grid differs from the screen's:
   resize the agent PTY + screen (mid-session resize is allowed — full PTY on
   the user's machine; TUIs repaint). If **no** session is live: leave the
   retained screen untouched.
5. Register the sender (drop same-session-id duplicates first, as §2.2).
6. Respond with the current snapshot/cursor and `live`.

**`agent.attach` never errors because no session is running** — `live: false`
with the last (or blank) snapshot is the contract. Only an unknown task errors.

`provider` is the harness the tab's PTY runs (`"claude"` / `"codex"`), and is
`null` when no agent has ever run in that worktree — the retained screen of an
exited agent is the only record of which harness painted it, so the Agent tab's
start offer leads with that provider rather than a guess.

### 3.3 The agent pump

`spawn_agent_pump(state, task_id, generation, rx)` — same body as §2.4 against
`agent_screens.get_mut(&task_id).filter(|a| a.pumped_generation == generation)`
(a newer pump supersedes; the stale task returns). Push `term_id` is
`format!("agent:{task_id}")`. Differences from user pumps:

- **Start-of-session reset:** before the loop, under the lock, reset the
  parser to a blank screen of the current grid
  (`screen.parser = vt100::Parser::new(rows, cols, 2000)`) and push
  `term.reset` with the blank snapshot and the current (cumulative) cursor —
  clients wipe and the new session's output starts clean. `total` is NEVER
  reset (cursor monotonicity is what client dedupe rides on).
- On `RecvError::Closed` (the phase ended / harness exited): do **not** remove
  the screen. Set `live = false`, flush any `pending`, push
  `term.closed { term_id: "agent:<task_id>", reason: "agent_session_ended" }`,
  return. The retained parser is the "last screen" the tab shows.

### 3.4 Sessions starting while attached

A viewer staring at the Agent tab must see a session that starts *after* they
attached (approve plan → build session spawns). Hook: at the tail of
`finish_mutation`, after the reaper call (§2.7), run:

```rust
fn ensure_agent_pumps(&mut self, state_arc: &Arc<Mutex<AppState>>)  // conceptual
```

— for every `agent_screens` entry with a non-empty `attached` whose task has a
live session with generation != `pumped_generation`, start a pump. Because
`finish_mutation` is `&mut self` and pump-spawning needs the `Arc`, implement
this as: `finish_mutation` returns/records the task ids needing pumps and the
`dispatch_frame`-level callers (which own the Arc) start them — OR keep an
`Arc<Mutex<AppState>>` self-handle set once at `shared()` time
(`Weak<Mutex<AppState>>` field) and upgrade it inside. **Pick the Weak-handle
approach** (`self_handle: Option<std::sync::Weak<Mutex<AppState>>>`, set in
`AppState::shared()`): it keeps the hook self-contained, and `dispatch` paths
that run in tests without an Arc simply skip pump spawning (they assert on
state, not pushes). Document the field with exactly this rationale.

Sessions that *end* need no hook — the pump sees `Closed` on its own.

### 3.5 QA mode

The QA warm double (`sh -c 'cat >/dev/null'`) is a real PTY: `agent.attach`
returns `live: true` while a phase is in flight, input drains without
blocking, output is empty. Tests must not wait for output from the double —
assert on the attach response shape and on `term.closed
{reason:"agent_session_ended"}` after the scripted `done` ends the session.

---

## 4. `fs.tree` / `fs.read` (bridge, Layer 2)

Both ride the app session, both parse the scope per §1, both fence with
`crate::task::is_worktree_contained_path` **plus** the canonical-containment
check below. New module-level constants in `app.rs`:
`FS_READ_MAX_BYTES: u64 = 1_048_576` (1 MiB).

### 4.1 Fencing (both RPCs)

1. `path` param: optional for `fs.tree` (absent/`""` = scope root), required
   for `fs.read`. When non-empty it MUST pass `is_worktree_contained_path`
   (relative, Normal components only) → else `"path escapes the worktree"`.
2. Join onto the resolved scope root, then **canonicalize both** the joined
   path and the scope root (`std::fs::canonicalize`); the canonical target
   must `starts_with` the canonical root → else the same error. This is what
   defeats symlinks pointing outside the worktree — the lexical fence alone
   cannot (a symlink's own components are all Normal).
3. `fs.read` additionally refuses to read through a symlink *leaf*:
   `std::fs::symlink_metadata` on the joined path; if it is a symlink, error
   `"refusing to read a symlink"`. (Directory-level symlink traversal is
   already caught by step 2.)

### 4.2 `fs.tree`

`{ task_id? | project_id [+ worktree_id], path? }` →

```json
{
  "path": "src/views",
  "entries": [
    { "name": "sheets", "kind": "dir" },
    { "name": "app.js", "kind": "file", "size": 2048 },
    { "name": "link",   "kind": "symlink" }
  ]
}
```

One directory level per call. Rules:

- `kind` from `symlink_metadata` (never follows): `"dir"`, `"file"`,
  `"symlink"`. `size` only for `"file"`.
- Skip any entry named `.git` (root worktree gitlink file and submodule dirs
  alike). Everything else — including `.build` and dotfiles — is listed.
- Sort: dirs first, then files+symlinks, each group case-insensitive by name.
- `path` in the response echoes the (relative) request path, `""` for root.
- A `path` that is not a directory → `"not a directory"`.

### 4.3 `fs.read`

`{ task_id? | project_id [+ worktree_id], path }` →

```json
{
  "path": "README.md",
  "size": 5120,
  "truncated": false,
  "mime": "text/markdown",
  "content_b64": "<b64>"
}
```

- `size` is the file's real length in bytes; `content_b64` carries at most the
  first `FS_READ_MAX_BYTES` bytes; `truncated = size > FS_READ_MAX_BYTES`.
  (~1.37 MiB of b64 in one response frame is acceptable as a request/response;
  pushes are the flood-sensitive path, not calls.)
- Content is ALWAYS base64 (text included) — byte-exact for any file.
- `mime` is an extension-based hint, lowercased extension via this pinned
  table (a helper `fn mime_hint(path: &Path, head: &[u8]) -> &'static str`):

| extensions                               | mime                       |
|------------------------------------------|----------------------------|
| md, markdown                             | `text/markdown`            |
| html, htm                                | `text/html`                |
| svg                                      | `image/svg+xml`            |
| png                                      | `image/png`                |
| jpg, jpeg                                | `image/jpeg`               |
| gif                                      | `image/gif`                |
| webp                                     | `image/webp`               |
| ico                                      | `image/x-icon`             |
| bmp                                      | `image/bmp`                |
| json                                     | `application/json`         |
| pdf                                      | `application/pdf`          |
| (no match, NUL byte in first 8 KiB)      | `application/octet-stream` |
| (no match, otherwise)                    | `text/plain`               |

- Reading a directory → `"not a file"`. Missing file → the io error with
  context (`format!("cannot read {path}: {e}")`).

---

## 5. Primary-checkout surface (bridge, Layer 2)

### 5.1 `diff.rs`: the uncommitted-changes diff

```rust
/// The primary checkout's uncommitted delta: HEAD's tree vs the working
/// directory and index, untracked included — staged + unstaged + new files.
/// This is the "main worktree" review surface; committed work is upstream's
/// business, not a review surface.
pub fn diff_against_head(repo_path: &Path) -> Result<WorktreeDiff, DiffError>
```

Implementation: open repo, `repo.head()?.peel_to_tree()?`, then the shared
`diff_tree_to_dirty_workdir`. An unborn HEAD (fresh empty repo) surfaces
git2's error — callers degrade (below). `.build/mcp.json` exclusion comes free
from the shared tail.

### 5.2 `project.diff` RPC (app session)

`{ project_id }` →

```json
{
  "project_id": "proj-1",
  "branch": "main",
  "path": "/Users/z/dev/repo",
  "stat": { "files_changed": 2, "insertions": 10, "deletions": 3 },
  "files": [ { "path": "src/a.rs", "status": "Modified" } ],
  "patch": "diff --git …"
}
```

Mirrors `worktree.diff`'s shape (same `files`/`stat`/`patch` field names so
`parseDiff`/`diffFilesHtml` reuse is mechanical). `branch` =
`repo.head()?.shorthand()` (`"(detached)"` handling is the SPA's, which
already does it for worktrees; send the shorthand or `"HEAD"` as git gives
it). Unknown project → `"unknown project_id"`; a diff failure is a clean error.

### 5.3 The cached primary summary in `task.list`

`Project` gains `primary_summary: Option<(std::time::Instant, Value)>` and the
constant `PRIMARY_SUMMARY_TTL: Duration = Duration::from_secs(10)` (same
discipline as `EXTERNAL_SCAN_INTERVAL` / `TASK_STAT_TTL`). `task.list`'s
response gains a third top-level array:

```json
{
  "tasks": [ … ],
  "external_worktrees": [ … ],
  "primary_changes": [
    {
      "project_id": "proj-1",
      "branch": "main",
      "files_changed": 2,
      "insertions": 10,
      "deletions": 3
    }
  ]
}
```

Computed per project by `fn primary_changes_json(&mut self) -> Vec<Value>`:
serve the cache when younger than the TTL; otherwise run
`diff_against_head(&project.repo_path)` + `head().shorthand()`, cache, return.
A per-project failure (unborn HEAD, fs error) logs via `eprintln!` and
contributes nothing (same posture as the external scan). No cache
invalidation hooks — 10 s staleness on a passive summary is the accepted
contract, identical to task stats.

### 5.4 Entry points (SPA shapes, implemented in Layer 4)

- **Sidebar** (`core/sidebar.js`): each project block gains, directly above
  the worktree line, a `main` row:
  `⌂ main` + the branch name + `· N uncommitted` (dirty count =
  `files_changed`) when non-zero, wired to `#/main/<projectId>`. Model change:
  `buildSidebarModel` accepts `primaryChanges` and attaches
  `m.primary = {branch, files_changed, insertions, deletions} | null`.
- **Project page** (`views/project.js`): a `MAIN` bucket above WORKTREES with
  one card: branch name, `path`, `+N −M` when dirty, → `#/main/<projectId>`.
- **Feed** (`core/taskFeed.js`): snapshot gains
  `primaryChanges: list.primary_changes || []`.

---

## 6. SPA terminal client rework (Layer 3)

### 6.1 `spa/src/terminal/session.js` → the multiplexed socket

`TerminalSession` is reworked in place (same file, class renamed
**`TerminalSocket`**). Everything about the E2EE bootstrap, pinned-key check,
liveness ping, backoff reconnect, and generation guard is kept verbatim. What
changes:

- `start(cols, rows)` → `start()` — connecting no longer implies attaching.
- A registry `this._terms = new Map()` of
  `termId → { kind: "user"|"agent", taskId?, cols, rows, lastCursor, onOutput, onSnapshot, onClosed, onLive }`.
- `_demux` routes by `p.term_id`: `term.output` / `term.reset` apply the
  per-term cursor dedupe then call that term's `onOutput`/`onSnapshot`;
  `term.closed` calls `onClosed(reason)` (and for `kind:"agent"` with
  `reason:"agent_session_ended"` it does NOT deregister — the tab keeps its
  screen; user terms deregister).
- Public API (all return promises; `_call` unchanged underneath):

```js
async createTerminal(scope, cols, rows)        // → { term_id }  (term.create)
async listTerminals(scope)                     // → [ {term_id, cols, rows} ] (term.list)
async closeTerminal(termId)                    // term.close + deregister
async attachTerminal(termId, opts)             // register + term.attach → snapshot applied via opts.onSnapshot
async attachAgent(taskId, opts)                // register "agent:<taskId>" + agent.attach; opts.onLive(live)
detach(termId)                                 // deregister only (tab unmounted; server keeps the PTY)
async input(termId, data)                      // term.input
async resize(termId, cols, rows)               // term.resize
```

- **Reconnect re-attach:** after the handshake, instead of the old single
  `term.attach`, iterate `this._terms`: user terms → `term.attach` (a
  rejection with `unknown term_id` → treat as closed: `onClosed("reaped")` +
  deregister); agent terms → `agent.attach` (update `onLive`). Each response's
  snapshot flows through `onSnapshot` with `lastCursor` reset to the response
  cursor first.
- `scope` is the plain `{task_id}` / `{project_id, worktree_id}` /
  `{project_id}` object spread into params.

### 6.2 `spa/src/terminal/manager.js` (new)

The per-browser-tab singleton owner:

```js
export function terminalManager()          // lazy-create the TerminalSocket (RELAY_URL,
                                           // fetchGatewayToken, pinnedDeviceTransportKey,
                                           // preferDeviceId — exactly the drawer's old wiring)
export function retargetTerminals()        // drawer.js's retargetTerminal logic, verbatim
```

`connection.js` swaps `retargetTerminal` → `retargetTerminals` (same call
site, `switchDevice`). The socket is created on first use (first terminal/agent
tab mounted), never at boot — no terminal socket for users who never open one.

### 6.3 `spa/src/terminal/pane.js` (new) — the reusable ghostty tab component

```js
export async function mountTerminalPane(host, {
  attach,            // (opts) => attach promise: manager attachTerminal/attachAgent bound
  input,             // (data) => promise
  resize,            // (cols, rows) => promise
  onExit,            // (reason) => void   — tab-level reaction (close tab / show quiet chip)
}) → { dispose(), fit(), terminal }
```

Behavior (ported from `drawer.js`, generalized):

- Lazy `import("ghostty-web")` + `init()` once per page (module-level promise).
- `new Terminal({ fontSize: 13, theme: { background: "#15161e", foreground: "#a9b1d6" } })`,
  `FitAddon`, and the **exact** `fitTerminalToViewport` strategy documented in
  drawer.js (proposeDimensions + direct `term.resize`, never `fit()`, skip
  when unchanged) — keep that comment with the code.
- `onSnapshot`: `term.reset(); term.write(bytes)`. `onOutput`: `term.write`.
- `term.onData → input(...)` (`.catch(() => {})`), `term.onResize → resize(...)`
  (`.catch(console.warn)` — the drawer's rationale comment survives).
- `ResizeObserver` on `host` + window-resize listener; both removed in
  `dispose()`. `dispose()` also calls the caller-provided detach (via
  `onExit` wiring at the call site) — **dispose never closes the server PTY**.
- QA hook: on mount (and on focus) set `window.__buildTerminal = terminal`
  (the harness pokes the most recently mounted pane).

### 6.4 Drawer removal (complete list)

- **Delete** `spa/src/terminal/drawer.js`.
- `spa/index.html`: remove the whole `<div id="drawer">…</div>` block
  (lines ~40–43, including `#dtitle`/`#dx`/`#term`).
- `spa/src/styles.css`: remove the `/* terminal drawer */` section
  (`#drawer`, `#drawer.show`, `#drawer .dh`, `#drawer .dh .x`, `#term` rules,
  and the mobile `#drawer { height:62vh; }` override). Add the tab-pane styles
  (§7) in their place.
- `spa/src/main.js`: remove `initTerminalDrawer` import + call (the backtick
  keybinding dies with it — no replacement keybinding).
- `spa/src/views/task.js`: remove the `toggleTerminal` import and the
  `#termToggle` header chip (`terminal \``) + its `onclick`.
- `spa/src/connection.js`: swap the `retargetTerminal` import for
  `retargetTerminals` from `manager.js`.
- `web/feature-check.mjs` section *g* and `web/mobile-check.mjs` section 8:
  rewrite against the terminal *tab* (open a task → `+` a terminal → same
  `window.__buildTerminal` cols/rows refit assertions). Layer 4 owns this.

---

## 7. SPA tab shell, Files tab, surfaces (Layer 4)

### 7.1 `spa/src/core/tabshell.js` (new)

A DOM-light helper that renders the `.tabs` row and owns tab wiring —
**content painting stays with the views**:

```js
export function mountTabShell(host, {
  tabs,            // [{ id, label, closable?: bool }] — static tabs first
  active,          // current tab id
  onSelect,        // (id) => void
  onClose,         // (id) => void        (closable tabs' ×)
  onNewTerminal,   // () => void | null   (renders the "+" when provided)
}) → { setActive(id), setTabs(tabs) }
```

Markup contract: reuse the existing `.tabs > .t.active` classes (CSS already
styles them); closable tabs append `<span class="tx">×</span>`; the `+` is
`<div class="t tplus">+</div>`. New CSS: `.t .tx` and `.tplus` only.

### 7.2 Terminal tabs behavior (all three surfaces, identical)

- On view mount: `terminalManager().listTerminals(scope)` → one tab per
  terminal, labeled by ordinal (`1`, `2`, …), id = `term_id`.
- `+`: `createTerminal(scope, 80, 24)` then select the new tab (the pane's
  first fit immediately resizes to the real grid).
- Selecting a terminal tab mounts a `pane.js` pane into `#tabbody` and
  attaches; leaving the tab `dispose()`s the pane and `detach()`s (PTY lives
  on). Re-entering re-attaches (snapshot resync).
- `×` on the tab: `closeTerminal(termId)`, drop the tab, select the surface's
  default tab.
- `onClosed("exited"|"reaped"|"closed")` while mounted: drop the tab and
  select the default tab (no error banner — an exited shell is normal).
- **Poll interaction (binding):** the existing 1.6 s `paint()` loops must
  early-return for tab ids other than `plan`/`diff`/`changes` after the shell/
  banner upkeep — Files, Agent, and terminal tabs are push- or fetch-driven
  and are NEVER innerHTML-wiped by the poll.

### 7.3 Task view (`views/task.js`)

- Tabs: `Plan`, `Diff`, `Files`, `Agent`, terminals…, `+` (per §0.8 the
  default-active rule; plan/diff content code paths untouched, including
  `paintStages`).
- Route: `#/task/<id>/<tab>` where tab ∈
  `plan | diff | files | agent | term-<n>`; router maps unknown → `plan`
  (replacing today's binary diff/plan check). `hashFromRoute` unchanged in
  shape.
- **Agent tab:** mounts a pane with `attachAgent(taskId, …)`. When the attach
  response (or a later `onLive(false)` / `term.closed
  {reason:"agent_session_ended"}`) says no live session, render a quiet header
  chip over the pane: `no active agent session` (class `agent-idle`), keeping
  the last screen visible. Input while dead surfaces the RPC error in that
  chip (`no active agent session`), not a banner. Attach errors for an
  *unknown task* render the chip alone. The tab NEVER breaks the rest of the
  view.
- Scope for terminals + Files: `{ task_id: id }`.

### 7.4 Files tab (`spa/src/views/files.js`, new — shared by all 3 surfaces)

```js
export function renderFilesTab(body, { scope, callRpc })
```

Layout: `.files` = flex row; left `.ftree` (one-level-at-a-time directory
listing with a `..` row when below the root and a breadcrumb of the current
relative path), right `.fpreview`. On phones (< 900px) they stack. Tree
entries: dir rows navigate (`fs.tree`), file rows preview (`fs.read`),
symlink rows render but do nothing (title: `symlink — not followed`). All
names/paths through `esc()`.

Preview rules by response `mime` (+ `truncated`):

| mime               | rendering                                                                             | view-source toggle |
|--------------------|----------------------------------------------------------------------------------------|--------------------|
| `text/markdown`    | `renderMarkdown(text)` in a `.plan`-styled div                                          | yes                |
| `text/html`        | `<iframe class="fhtml" sandbox="" src="data:text/html;base64,<content_b64>">`           | yes                |
| `image/svg+xml`    | `<img src="data:image/svg+xml;base64,<content_b64>">` — never inline SVG into the DOM   | yes                |
| other `image/*`    | `<img src="data:<mime>;base64,<content_b64>" style="max-width:100%">`                   | no                 |
| `application/octet-stream`, `application/pdf` | placeholder: `binary file · <size> bytes` — no content render | no                 |
| everything else    | source view                                                                             | (is source)        |

- **Source view** = `<pre class="fsrc"><code>` of the UTF-8-decoded,
  `esc()`-escaped bytes.
- The toggle is a small `view source` / `view rendered` button in the preview
  header (which also shows the escaped path + size).
- `truncated: true`: text/source views render what arrived with a
  `truncated at 1 MiB` notice; image/html/svg views show the binary-style
  placeholder `file too large to preview · <size> bytes` instead (a partial
  image/document is garbage).
- The iframe sandbox attribute is **exactly** `sandbox=""` — no
  `allow-scripts`, no `allow-same-origin`. The `data:` URL src (not
  `srcdoc`) keeps it origin-less in every browser.
- No polling. Fetch on navigation/selection only.

### 7.5 Main-worktree surface (`spa/src/views/mainWorktree.js`, new)

- Route `#/main/<projectId>` or `#/main/<projectId>/<tab>`, tab ∈
  `changes | files | term-<n>`, default `changes`. Router:
  `{ name: "main", projectId, tab }`; `hashFromRoute` emits the same. `go`
  dispatch added in `app.js` `render()`.
- Header: `← <project name>` back to the project page; `<h1>` = branch name;
  meta line = escaped repo path; chip `MAIN`.
- **Changes tab:** the git surface (`core/gitPane.js` + `core/gitRender.js`) —
  commit history (`git.log`/`git.show`) plus per-file staging
  (`git.stage`/`git.unstage`) and a commit-message box (`git.commit`) for the
  user's OWN work. The same pane mounts on both the primary checkout
  (`{ project_id }`) and task worktrees (task view's `Changes` tab, between
  Diff and Files, `{ task_id }`). It still carries NO review/comment
  affordances — review lives on the Diff tab — and a commit never advances a
  task past any gate. Task scope adds agent-commit options to the commit
  split button: "Ask agent to commit" (the exact canned `AGENT_COMMIT_MESSAGE`
  through the existing `task.message` verb, offered only in the messageable
  states) and "Commit all (Build message)" (the existing
  `task.git_action { action: "commit" }` verb) — no new bridge surface for
  either. Poll 1.6 s with the standard key-diff freeze (key = HEAD + status
  patch + per-file stage states + visible commits; also frozen while a commit
  message is being drafted).
- **Repo controls (Changes tab, both scopes):** a `.gittoolbar` above the file
  list surfaces the everyday repo verbs so review never needs a terminal —
  Fetch, a Pull split button (fast-forward primary; merge / rebase in the menu),
  a Push split button (force-push-with-lease behind the menu), ahead/behind
  chips from `git.status`, and a Stash split button (Stash / Pop, with the
  stash count badged). Branch switching (list, checkout, create, delete via
  `git.branches`/`git.checkout`/`git.branch_delete`) lives on the branch button
  and is **main-worktree only** — a task worktree's branch is owned by the task
  lifecycle, so sessions show the branch as static text. A `.gitstate` banner
  appears when the repo is mid-merge/rebase, with an Abort (`git.merge_abort`).
  Each uncommitted file gains a discard affordance (`git.discard`). Every
  destructive verb (discard, force push, branch delete, abort) is a two-click
  inline confirm — never a browser dialog — and every control routes through the
  same in-flight freeze as staging/commit. All new `git.status` fields
  (`repo_state`, `upstream`, `ahead`/`behind`, `stash_count`) degrade to hidden
  when an older bridge omits them.
- Files + terminals: scope `{ project_id: projectId }`.

### 7.6 External-worktree view (`views/worktree.js`)

- Gains the shell: `Diff` (the entire existing content — untouched, including
  adopt-on-action), `Files`, terminals…, `+`. Route becomes
  `#/worktree/<projectId>/<worktreeId>/<tab?>`, tab ∈
  `diff | files | term-<n>`, default `diff` (old 2-segment URLs keep working —
  the missing segment defaults).
- Scope: `{ project_id, worktree_id }`.
- The existing shell-freeze discipline stays; the tab row renders in `shell()`
  above `#tabbody`.

### 7.7 Sidebar + project page

Per §5.4. `views/sidebar.js` wires `[data-main]` rows → `go({ name: "main",
projectId })`. `core/sidebar.js` model/HTML changes are pure and unit-tested
in `spa/test/sidebar.test.js`.

---

## 8. QA + harness (Layer 4)

The scripted QA agent needs **nothing new**: real bash serves user terminals
in QA mode, and `agent.attach` works against the stdin-draining double (§3.5).

`web/client.mjs` gains a push-tolerant session:

```js
export async function openPushSession({ send, recv, transport, preferDeviceId, onPush })
```

— same bootstrap as `openSession`, but a background receive loop routes
decrypted payloads with `id`+`ok` to pending calls and payloads with `type`
(term.output / term.reset / term.closed) to `onPush`. qa.mjs opens a **second
relay connection** with it for the terminal checks, mirroring production's
dedicated terminal socket (the main `call` session stays strictly
request-response).

`web/qa.mjs` appends (after the existing 35 checks, before `ws.close()`):

1. **Keyed terminal round-trip** (primary scope): `term.create
   { project_id, cols: 80, rows: 24 }` → id matches `/^term-\d+$/`;
   `term.attach { term_id, cols: 80, rows: 24 }` → has `snapshot` + numeric
   `cursor`; `term.input` with `echo qa-term-<ts>\r`; collect `term.output`
   pushes (matching `term_id`) until the marker echoes back (10 s deadline);
   `term.list { project_id }` includes the id; `term.close { term_id }` →
   `term.list` no longer includes it.
2. **fs round-trip** (task scope, using the standard task from earlier in the
   run): `fs.tree { task_id }` → entries include `.build` and do NOT include
   `.git`; then take the first stage's `path` from `task.stages` and check
   `fs.read { task_id, path }` content (b64-decoded) equals
   `task.stage_doc`'s `contents`.
3. **fs fencing**: `fs.read { task_id, path: "../../../etc/passwd" }` rejects
   with `path escapes`.
4. **Primary summary**: `task.list` → `primary_changes` contains an entry for
   the project with a non-empty `branch` and numeric `files_changed`; and
   `project.diff { project_id }` returns the `stat`/`files`/`patch` shape.
5. **Agent attach**: on a freshly dispatched (working) task,
   `agent.attach { task_id }` → `term_id === "agent:<task_id>"` and `live`
   is boolean; on a merged task from earlier, attach still succeeds with
   `live: false`.

`web/terminal.mjs` + `web/term-verify.mjs` (node terminal clients): update to
the keyed flow (`term.create` against a scope from `project.list`, then
keyed attach/input). `web/feature-check.mjs` g + `web/mobile-check.mjs` §8:
per §6.4.

---

## 9. Layered task list (four sequential implementation agents)

Gates for EVERY commit in every layer:
`cd bridge && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt`
and `cd spa && npm test`, plus `semgrep --config auto` on changed files and
`gitleaks protect --staged`. TDD: each bullet's test lands (and fails) before
its implementation. Commit granularly on `worktree-surfaces` with the required
trailers.

### Layer 1 — bridge: keyed terminals + agent attach

Files: `bridge/src/app.rs` (TermScope, TermScreen, TermSession, terms map,
RPCs, pumps, reaper, drop_session), `bridge/src/orchestrator.rs`
(`session_generation`, `subscribe_with_generation`, `write_input_strict`),
`bridge/src/main.rs`/`service.rs` (spawn_terminal_reaper wiring).

Tests first (in `app.rs` `#[cfg(test)]`, following the existing
`term_attach`-style tests around line ~5150):
- scope parse table (all four rows of §1) + resolution errors.
- create/list/close round-trip per scope kind; list is scope-filtered and
  ordered; close pushes `term.closed{reason:"closed"}` to a detached sender
  and kill_and_reaps (assert via `pid()`→`has_exited` on the session).
- cap: 17th create fails with the pinned message.
- keyed attach snapshot/cursor semantics (port the two existing term tests to
  keyed ids); attach on unknown id errors; attach on `agent:` id errors.
- input/resize route by id; un-keyed input/resize error.
- pump EOF removes the entry and pushes `term.closed{reason:"exited"}`
  (spawn `sh -c exit` style shell… use a terminal whose bash gets `exit\r`).
- reaper: task-scope terminal closes after task.delete / abandon(prune);
  survives merge cleanup=keep; external-scope closes when the dir is removed;
  `finish_mutation` triggers it.
- agent.attach: unknown task errors; live:false + blank snapshot with no
  session; live:true while a QA phase runs; input via `agent:` id reaches the
  PTY (`write_input_strict` error when dead); resize dead → `live:false`
  no-op; `term.closed{reason:"agent_session_ended"}` after the scripted done;
  new session after attach starts a pump (ensure_agent_pumps via the Weak
  handle — test through `shared()`).
- drop_session detaches from every term + agent screen.

Do NOT touch: `stream.*` (leave the demo log alone), `fs.list`, diff.rs,
task lifecycle logic, the SPA, `web/`.

### Layer 2 — bridge: fs + primary surface + task.list summary

Files: `bridge/src/diff.rs` (`diff_against_head` + tests), `bridge/src/app.rs`
(`fs.tree`, `fs.read`, `mime_hint`, `project.diff`, `primary_changes_json`,
`Project.primary_summary`, dispatch entries).

Tests first:
- `diff_against_head`: clean repo → empty; staged+unstaged+untracked counted;
  mcp.json excluded (mirror the existing diff tests).
- fs.tree: one level, `.git` skipped, dirs-first ordering, kinds, `..`-free
  contract (escape path errors), not-a-directory error, scope reuse across
  all three scope kinds.
- fs.read: content round-trip, size/truncated at a >1 MiB fixture, mime table
  spot checks (md/svg/png/unknown-text/unknown-binary via NUL), symlink-leaf
  refusal, symlink-dir escape refusal (create a symlink to a tempdir outside
  the root; canonical check catches it), lexical escape refusal.
- `project.diff` shape + unknown project error.
- `task.list.primary_changes`: present per project, cached (mutate the repo,
  re-list within TTL → stale; force by constructing with a tiny TTL? No —
  follow the existing TASK_STAT_TTL test pattern in
  `task_views_carry_timestamps_and_a_cached_diffstat`).

Do NOT touch: terminal code from Layer 1, orchestrator, SPA, `web/`.

### Layer 3 — SPA: terminal client rework + drawer removal

Files: `spa/src/terminal/session.js` (→ TerminalSocket),
`spa/src/terminal/manager.js` (new), `spa/src/terminal/pane.js` (new),
DELETE `spa/src/terminal/drawer.js`, `spa/index.html`, `spa/src/styles.css`,
`spa/src/main.js`, `spa/src/connection.js`, `spa/src/views/task.js`
(chip removal ONLY — the tab shell is Layer 4's), tests
`spa/test/terminal.test.js` (rewrite for TerminalSocket).

Tests first (vitest, FakeWebSocket pattern already in terminal.test.js):
- handshake unchanged (pinned-key mismatch still hard-fails — keep those two
  tests, adapted).
- attachTerminal registers + sends `term.attach {term_id, cols, rows}`;
  output/reset demux by term_id with per-term cursor dedupe; frames for
  unregistered ids are ignored.
- two terminals demux independently over one socket.
- term.closed → onClosed + deregistration (user) vs retained (agent
  session-ended).
- reconnect re-attaches every registered term (user via term.attach, agent
  via agent.attach) and resets cursors from the new snapshots; an
  `unknown term_id` rejection on re-attach → onClosed("reaped").
- createTerminal/listTerminals/closeTerminal param shapes (scope spread).

The app must build and behave with the drawer gone (no dangling imports; grep
for `drawer`, `toggleTerminal`, `initTerminalDrawer`, `#drawer`, `#dx`,
`__buildTerminal` in spa/src must come back clean except pane.js's QA hook).

Do NOT touch: bridge code, `views/worktree.js`, `views/project.js`,
`core/router.js`, `web/` harnesses (Layer 4 rewrites the two drawer-dependent
sections; until then those two checks are known-red in the browser harnesses
only — the required gates are cargo + vitest).

### Layer 4 — SPA: tab shell + Files + surfaces + entry points + qa.mjs

Files: `spa/src/core/tabshell.js` (new), `spa/src/views/files.js` (new),
`spa/src/views/mainWorktree.js` (new), `spa/src/views/task.js`,
`spa/src/views/worktree.js`, `spa/src/views/project.js`,
`spa/src/core/router.js`, `spa/src/core/sidebar.js`,
`spa/src/views/sidebar.js`, `spa/src/core/taskFeed.js`, `spa/src/app.js`
(render dispatch for `main`), `spa/src/styles.css` (tabs ×/+, .files/.ftree/
.fpreview/.fhtml/.fsrc, .agent-idle), `web/client.mjs` (openPushSession),
`web/qa.mjs` (§8 appends), `web/feature-check.mjs`, `web/mobile-check.mjs`,
`web/terminal.mjs`, `web/term-verify.mjs`, new vitest files
`spa/test/tabshell.test.js`, `spa/test/files.test.js` (preview-rule selection
as pure functions — extract `previewModeFor(mime, truncated)`),
`spa/test/router.test.js` (extend).

Tests first:
- router: all new routes/tab segments both directions (main, task tabs,
  worktree tab, defaults, unknown-tab fallback).
- tabshell: renders tabs/active/×/+; callbacks fire; setTabs preserves active.
- files preview-rule table (§7.4) incl. truncated demotions; b64→text decode
  helper; escaping of names (a `<img src=x onerror>` filename renders inert).
- sidebar model: `primary` attachment + dirty-count rendering (extend
  sidebar.test.js).
- Full suites green, then the live harness: `web/qa.mjs` (35 existing + new
  checks all green), `feature-check.mjs`, `mobile-check.mjs` against compose.

Do NOT touch: bridge code; plan/diff/stages painters' internals (only their
mounting into the shell); the adoption flow logic in worktree.js.

---

## 10. Security checklist (must hold at every layer boundary)

1. fs scope roots resolved server-side from ids only — grep-provable: no
   `params` path ever feeds `resolve_root`.
2. `is_worktree_contained_path` on every client-supplied relative path
   (fs.tree path, fs.read path) + canonical-prefix containment + symlink-leaf
   refusal on reads (§4.1).
3. `fs.read` size cap enforced server-side (never trust the client to stop).
4. HTML preview: `sandbox=""` iframe over a `data:` URL; SVG via `<img>`
   only; no user bytes ever `innerHTML`-ed unescaped (files.js uses `esc()`
   everywhere; markdown goes through the already-escaping `renderMarkdown`).
5. Terminal scope validated before create; term_id existence before
   attach/input/resize/close; `agent:` ids gated per §0.2. Ownership beyond
   that is the E2EE session boundary (single-user device), unchanged.
6. Terminal cap + reaper bound resource growth; every close path
   `kill_and_reap`s.
7. All worktree/project/branch/file strings rendered in the SPA remain
   UNTRUSTED and escaped (existing convention).
