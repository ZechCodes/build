# Worktree Adoption — Binding Technical Spec

**Status:** Binding. Implementers follow this exactly; names and shapes below are the
contract between four sequential implementation layers. Where this spec pins a name,
use that name. Where behavior is unspecified, match the existing code's conventions
(fail fast, pure domain core, `#[serde(default)]` forward-compat, TDD, escape
everything rendered).

**Feature summary.** The bridge discovers git worktrees it did not create
("external worktrees") per project, surfaces them as understated cards on the
board, and lets the user browse their diff read-only. The first mutating action
*adopts* the worktree: a Quick-kind task is minted around it (landing directly in
`Review` — there is work to review), a checkpoint commit makes any pre-Build dirty
state legible, and `.build/mcp.json` is scaffolded so later agent sessions report
`done`. Adopted tasks get GitHub-style split-button verbs (Merge & clean up /
keep / release; Release vs Abandon & delete) so pruning the user's worktree is
always the user's explicit choice. The first agent session on an adopted task
continues the user's existing Claude Code conversation when a transcript exists
for that worktree.

---

## 0. Global decisions (read first)

1. **Zero changes to `bridge/src/task.rs`.** Adoption composes existing machinery:
   `Task::new(id, goal, TaskKind::Quick)` + `apply(Dispatch)` (→ `Building`) +
   `apply(BuildReady)` (→ `Review`). No new `TaskState`, no new `TaskEvent`, no
   new `TaskKind`. An adopted task is a Quick task whose worktree pre-existed.
2. **Discovery is observation, never mutation.** The scan (`git worktree list
   --porcelain` + per-worktree summaries) writes nothing, and a broken individual
   worktree is skipped with an `eprintln!` — an observation surface polled every
   2.5 s must not take the board down because one stray checkout is corrupt.
3. **`worktree_id` is server-resolved.** Clients send only the stable id
   (`"wt-" + first 12 hex chars of sha256(canonical absolute path)`); the bridge
   resolves it against its own discovered list. A raw path in params is never
   accepted, so the RPC surface cannot be used to diff/adopt arbitrary host paths.
4. **The external diff anchors at the merge-base**, not the base tip. External
   worktrees may predate movement on the base branch; diffing against the tip
   (what `diff_against_base` does, correctly, for short-lived task worktrees)
   would drown the worktree's own work in upstream noise. The new
   `diff_against_merge_base` uses `merge-base(base_branch, HEAD)` as the old
   tree and the working directory + index (untracked included) as the new side —
   `git diff base...HEAD` semantics plus dirty state. Task diffs are unchanged.
5. **Pruning principle (verbatim, binding):** *"we only take automated actions on
   our worktrees; if the user set in motion the task that led to the automation,
   we trust the user."* Concrete gates:

   | Action | Trigger | May prune an adopted worktree? |
   |---|---|---|
   | `task.approve_merge` / `task.git_action` merge, `cleanup:"prune"` | user | **yes** (default; the split button surfaces the choice) |
   | `task.abandon` | user | **yes** (the adopted UI default is Release, not this) |
   | `task.delete` | user | **no** — delete removes the card, not the user's files (§5.7) |
   | boot recovery, worktree missing | automated | n/a (nothing on disk; record-only abandon, both kinds) |
   | boot recovery, repo missing | automated | **no** — adopted tasks park needs-attention instead of abandoning (§5.8) |
   | `prune_merged_worktree` | follows a user-chosen `cleanup:"prune"` | yes |
6. **`adopted: bool` is durable.** New field on `ActiveTask` and `PersistedTask`
   (`#[serde(default)]` → pre-existing task files load as `false`, the native
   path). It gates the prune rules above, the boot-recovery parking, the release
   verb, and the SPA's option sets.
7. **Session continuation is one-shot and harness-specific.** `ActiveTask` (and
   `PersistedTask`) carry `pending_continuation: bool`, set `true` at adoption and
   consumed (set `false`) by the *first* session spawn afterwards, whatever it is.
   At that spawn an injectable `TranscriptProbe` decides whether `--continue` is
   appended to the `claude` argv; no transcript → fresh session, silently. Later
   sessions are always fresh (the existing "cold agent" discipline). The QA agent
   (`Agent::Warm`) never sees the flag.

   > **SUPERSEDED 2026-08-31 — adoption no longer inherits the human's session;
   > Build cannot show its history.** A new agent's conversation view starts at
   > sequence 1, so an agent resumed onto the session the human was having
   > before the adoption answers out of a history no view of Build's holds. The
   > pickup is deleted with the flag: `pending_continuation` is gone from
   > `ActiveRun` and `PersistedRun`, and the spawn rule
   > (Agent Session Interface Spec §10.4) resumes only what Build named
   > (`--resume <id>`) or an agent's own recorded history (`--continue`).
   > `adopted: bool` (item 6) stays, for everything else it gates.
8. **The scan is cached per project at a 10 s cadence** (`EXTERNAL_SCAN_INTERVAL`),
   never per poll. `task.adopt` forces a fresh scan (adoption must see current
   bindings); adopt/release explicitly invalidate the cache so cards
   appear/disappear on the next poll.
9. **Backward compat.** Native task flows are byte-identical: `task.approve_merge`
   / `task.git_action` without `cleanup` behave exactly as today (prune after the
   persisted verdict), `task.abandon` is unchanged, old persisted records load
   with `adopted: false, pending_continuation: false`, `task.list` only *adds* the
   `external_worktrees` key, and every QA-agent flow keeps passing.

---

## 1. Discovery (`bridge/src/worktree.rs`)

### 1.1 New types

```rust
/// One git worktree of the project repo that Build did not create (or no longer
/// tracks): the raw material of adoption. Pure data — discovery never mutates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalWorktree {
    /// Stable id: "wt-" + the first 12 hex chars of sha256 over the canonical
    /// absolute path (UTF-8 bytes of `path.display().to_string()`).
    pub id: String,
    /// Git's internal worktree name (`repo.find_worktree(name)` works) — kept so
    /// adoption can build a `Worktree` that `WorktreeManager::remove` understands.
    pub name: String,
    /// Canonical absolute path of the working directory.
    pub path: PathBuf,
    /// Checked-out branch, or None for a detached HEAD (browsable, not adoptable).
    pub branch: Option<String>,
    pub head_sha: String,
    /// HEAD commit subject (`%s`). UNTRUSTED display text.
    pub head_subject: String,
    /// Seconds since the HEAD commit's committer time (clamped at 0).
    pub head_age_seconds: u64,
    /// `git status --porcelain` line count — staged + unstaged + untracked.
    pub dirty_files: usize,
    /// Roll-up of `diff_against_merge_base(path, base_branch)` (§2).
    pub diffstat: crate::diff::DiffStat,
}
```

`WorktreeError` gains one variant (existing variants untouched):

```rust
#[error("git command failed: {0}")]
Command(String),
```

### 1.2 The scan function

```rust
/// Enumerate every git worktree of `repo_path` that is neither the primary
/// checkout nor in `excluded_paths` (canonical paths of task-bound worktrees),
/// with a review summary per worktree. Read-only. A worktree whose summary
/// cannot be computed (corrupt checkout, no merge base with the base branch)
/// is skipped with an eprintln! — one broken stray must not fail the scan.
pub fn discover_external_worktrees(
    repo_path: &Path,
    base_branch: &str,
    excluded_paths: &std::collections::HashSet<PathBuf>,
) -> Result<Vec<ExternalWorktree>, WorktreeError>
```

Pinned behavior:

1. Run `git worktree list --porcelain` in `repo_path` via `std::process::Command`
   (a non-zero exit is `WorktreeError::Command` carrying trimmed stderr). Parse
   blank-line-separated blocks; per block the fields used are `worktree <path>`
   (first line), `HEAD <sha>`, and `branch refs/heads/<name>` **or** `detached`.
2. Skip: blocks with `bare` or `prunable`; the block whose canonicalized path
   equals the canonicalized `repo_path` (the primary checkout — never listed,
   never adoptable); blocks whose path no longer exists on disk; blocks whose
   canonicalized path is in `excluded_paths`.
3. `branch`: strip the `refs/heads/` prefix; `detached` → `None`.
4. `name`: resolve via git2 — open the primary repo once, iterate
   `repo.worktrees()`, match `find_worktree(n).path()` (canonicalized) against
   the block's path. No match → skip with `eprintln!` (cannot happen for
   worktrees the porcelain listing just reported, but never panic on git state).
5. `head_subject` / `head_age_seconds`: `repo.find_commit(head_sha)` →
   `summary()` (empty string when absent) and `now_unix - commit.time().seconds()`
   clamped at 0.
6. `dirty_files`: count of non-empty lines of `git -C <worktree> status
   --porcelain`.
7. `diffstat`: `crate::diff::diff_against_merge_base(&path, base_branch)?.stat()`
   — a per-worktree error here skips the worktree (rule 2 above).
8. Order: `head_age_seconds` ascending (most recently committed first), ties by
   path. Deterministic output for cache-key stability and tests.

The id helper is its own pinned pure function (unit-tested directly):

```rust
/// The stable external-worktree id for a canonical absolute path.
pub fn external_worktree_id(path: &Path) -> String // "wt-" + 12 hex of sha256
```

(Add `sha2` use in worktree.rs — already a crate dependency.)

### 1.3 Goal derivation (pure, pinned)

```rust
/// The silently derived goal for an adopted worktree: the branch name verbatim,
/// unless the branch is generic — then the HEAD commit subject.
pub fn derive_adoption_goal(branch: &str, head_subject: &str) -> String
```

Pinned rule:

1. Take the branch's final `/`-segment, lowercase it, strip one trailing run of
   digits and any single `-`/`_` immediately before that run (`wip-2` → `wip`,
   `test_3` → `test`).
2. It is **generic** iff the result is empty or one of:
   `main, master, dev, develop, wip, tmp, temp, test, testing, scratch, patch,
   fix, feature, new, branch`.
3. Not generic → return `branch` verbatim (the whole branch name, not the
   segment). Generic → return `head_subject` trimmed; if that is empty → return
   `branch`; if both are empty → `"Adopted worktree"`.

Pinned test cases: `("hotfix/login-redirect", _) → "hotfix/login-redirect"`;
`("wip", "Fix the thing") → "Fix the thing"`; `("wip-2", s) → s`;
`("ada/test_3", s) → s`; `("feature", s) → s`; `("", "") → "Adopted worktree"`.

### 1.4 Layer-1 tests (write first)

In `worktree.rs` tests (reuse `init_repo()`):

- `external_worktree_id_is_stable_and_prefixed` — same path twice → same id,
  starts with `wt-`, 15 chars total; different paths → different ids.
- `discovery_lists_a_user_worktree_and_skips_the_primary` — `git worktree add
  ../wt-a -b hotfix/thing`, commit + dirty file there → one entry with branch
  `Some("hotfix/thing")`, `dirty_files == 1`, non-empty `head_subject`, correct
  `name`; the primary repo path never appears.
- `discovery_excludes_bound_paths` — pass the worktree's path in
  `excluded_paths` → empty result.
- `discovery_reports_detached_head` — `git worktree add --detach ../wt-d` →
  entry with `branch: None`.
- `derive_adoption_goal_pinned_cases` — the table in §1.3.

---

## 2. Dirty diff variant (`bridge/src/diff.rs`)

### 2.1 New function

```rust
/// The worktree's total delta from its fork point with `base_branch`: the
/// merge-base tree vs the working directory *and* index, untracked included —
/// committed, staged, unstaged, and new files together. This is the browse/
/// review surface for external worktrees, which may long predate the base tip;
/// `diff_against_base` (base-tip-anchored) remains the task-diff surface.
pub fn diff_against_merge_base(
    worktree_path: &Path,
    base_branch: &str,
) -> Result<WorktreeDiff, DiffError>
```

Pinned implementation shape: open the repo at `worktree_path`; resolve
`base = revparse_single(base_branch).peel_to_commit()` and
`head = head().peel_to_commit()`; `let base_id = repo.merge_base(base.id(),
head.id())?` (no merge base — unrelated histories — surfaces the git2 error:
fail fast, the RPC error is legible); peel that commit to a tree; then the
**identical** diff/stat/patch construction `diff_against_base` uses. Extract the
shared tail into a private helper so the two public functions differ only in
which tree they pass:

```rust
fn diff_tree_to_dirty_workdir(
    repo: &git2::Repository,
    old_tree: &git2::Tree,
) -> Result<WorktreeDiff, DiffError>
```

`WorktreeDiff`, `DiffStat`, `ChangedFile`, `watch`, and every existing caller are
untouched.

### 2.2 Layer-1 tests (write first)

- `merge_base_diff_sees_committed_staged_unstaged_and_untracked` — branch off
  `main`, one committed file, one staged, one modified-tracked, one untracked →
  all four paths in `files()`, patch contains the untracked content.
- `merge_base_diff_ignores_base_movement` — branch off, commit `feature.rs` on
  the branch, then advance `main` with an unrelated commit →
  `diff_against_merge_base` reports exactly `feature.rs` (this is the case where
  `diff_against_base` would also report the upstream file as deleted).
- `merge_base_diff_on_detached_head_works` — detach at a commit, add a dirty
  file → diff includes it (browsing detached worktrees is legal).

---

## 3. Orchestrator (`bridge/src/orchestrator.rs`)

### 3.1 `ActiveTask` additions

```rust
pub struct ActiveTask {
    // ... existing fields unchanged ...
    /// True for a task minted around a pre-existing (user-created) worktree.
    /// Gates pruning (§0.5), release, and boot-recovery parking.
    pub adopted: bool,
    /// One-shot continuation flag (§0.7): set at adoption, consumed by the
    /// first session spawn afterwards.
    pub pending_continuation: bool,
}
```

`ActiveTask::reattach` gains the two parameters (`adopted: bool,
pending_continuation: bool`, appended after `comments` — it mirrors the persisted
record field-for-field; the `#[allow(clippy::too_many_arguments)]` stays).
`dispatch()` initializes both to `false`.

### 3.2 Spawn plumbing: `SpawnOptions`, `TranscriptProbe`, `spawn_session`

```rust
/// Per-spawn context a one-shot harness builder may honor.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpawnOptions {
    /// Resume the harness's own most-recent conversation for this cwd
    /// (claude: `--continue`). Set only for the first session after adoption.
    pub continue_session: bool,
}

/// Builds the one-shot harness command for a rendered prompt + model + context.
pub type OneShotBuilder =
    std::sync::Arc<dyn Fn(&str, &ModelChoice, &SpawnOptions) -> HarnessSpec + Send + Sync>;

/// Whether the harness has an existing conversation transcript for a worktree
/// cwd. Injectable so tests never touch the real home directory.
pub type TranscriptProbe = std::sync::Arc<dyn Fn(&Path) -> bool + Send + Sync>;
```

`Orchestrator` gains a field `transcript_probe: TranscriptProbe`, defaulting in
`Orchestrator::new` to `Arc::new(|_| false)` (never continue unless the app
layer opts in), plus a builder:

```rust
pub fn with_transcript_probe(mut self, probe: TranscriptProbe) -> Self
```

The private `fn spawn(&self, worktree, prompt, model_choice)` is **replaced** by:

```rust
/// Spawn the task's next session, consuming the one-shot continuation flag:
/// the first spawn after adoption probes for an existing harness transcript in
/// the worktree and asks the one-shot builder to continue it. Native tasks
/// (flag false) are byte-identical to today.
fn spawn_session(&self, active: &mut ActiveTask, prompt: &str) -> Result<(), OrchestratorError> {
    let continue_session =
        active.pending_continuation && (self.transcript_probe)(&active.worktree.path);
    active.pending_continuation = false;
    let options = SpawnOptions { continue_session };
    let session = match &self.agent {
        Agent::Warm(spec) => { /* unchanged: spawn + write_prompt; options ignored */ }
        Agent::OneShot(build) => {
            let spec = build(prompt, &active.model_choice, &options);
            PtySession::spawn(&spec, Some(active.worktree.path.clone()), self.pty_size)?
        }
    };
    active.session = Some(session);
    Ok(())
}
```

Every call site of the old `spawn` (`dispatch`, `on_stage_session_done`,
`approve_plan`, `dispatch_stage`, `send_notes`, `send_stage_notes`, `fix_stage`,
`request_changes`, `resume`) becomes `self.spawn_session(&mut active /*or
active*/, &prompt)?;` — the `active.session = Some(...)` assignments disappear.
Existing test closures (`recording_agent`, and the app.rs OneShot builders) gain
the third `&SpawnOptions` parameter.

### 3.3 `adopt`

```rust
/// Mint a Quick-kind task around an existing external worktree. No agent
/// session is spawned — the task lands in Review (there is work to review).
/// Order matters: checkpoint FIRST (pre-Build work stays its own legible
/// commit), then scaffold `.build/mcp.json` (left uncommitted, as on the
/// native path).
pub fn adopt(
    &self,
    id: TaskId,
    external: &ExternalWorktree,
    base_branch: &str,
    model_choice: ModelChoice,
) -> Result<ActiveTask, OrchestratorError>
```

Pinned steps (any error aborts with nothing persisted — the caller only persists
on `Ok`):

1. **Refuse detached HEAD:** `external.branch` is `None` →
   `Err(OrchestratorError::Gate("cannot adopt a detached-HEAD worktree — check out a branch first".to_string()))`.
2. **Refuse the base branch:** `branch == base_branch` →
   `Err(OrchestratorError::Gate(format!("cannot adopt a worktree with the base branch {base_branch:?} checked out")))`
   (merging a branch into itself is meaningless, and the primary checkout could
   never merge while its base is checked out elsewhere).
3. **Checkpoint commit:** `self.commit_all_with_message(&external.path,
   "Checkpoint: adopted by Build")?` — the exact message, no interpolation.
   Fires only when there is dirty state (the helper already no-ops on a clean
   tree). Refactor: the existing private `commit_all(path, goal)` becomes a
   one-line wrapper over the new
   `fn commit_all_with_message(&self, worktree_path: &Path, message: &str)`
   which holds the current add/status/commit body (`commit_all` passes
   `&format!("Build: {goal}")`).
4. Build `Worktree { name: external.name.clone(), path: external.path.clone(),
   branch, base_branch: base_branch.to_string() }`.
5. `self.scaffold_build_dir(&worktree, &id)?` (the existing function, unchanged).
6. `let goal = derive_adoption_goal(&branch, &external.head_subject);`
7. `Task::new(id, goal, TaskKind::Quick)`; `apply(TaskEvent::Dispatch)?`;
   `apply(TaskEvent::BuildReady)?` → `Review`.
8. Return `ActiveTask` with: `plan_path: DEFAULT_PLAN_PATH`, `last_summary:
   None`, `model_choice`, `last_error: None`, empty stages/comments, no session,
   `adopted: true`, `pending_continuation: true`.

Everything downstream (diff, request_changes, approve_merge, abandon, commit,
push) already works on the resulting `ActiveTask` unchanged. Note:
`orch.diff()` (task diff) keeps using `diff_against_base` for adopted tasks too —
once adopted, the branch is being driven toward a merge with the base tip, so
tip-anchored review is correct; the merge-base anchor is only for *browsing*
un-adopted worktrees.

Release needs no orchestrator method — it is record bookkeeping plus
`ActiveTask::end_session()`, both owned by the app layer (§5.6).

### 3.4 Layer-2 tests (write first)

- `adopt_lands_in_review_with_a_checkpoint_commit` — repo + `git worktree add
  ../wt -b user/thing`, dirty file → `adopt` → state `Review`, `adopted`,
  `pending_continuation`, kind `Quick`, goal `"user/thing"`,
  `.build/mcp.json` exists, `git log -1 --format=%s` in the worktree is
  `Checkpoint: adopted by Build`, and the checkpoint commit does **not**
  contain `.build/mcp.json` (checkpoint-before-scaffold order).
- `adopt_on_a_clean_worktree_makes_no_checkpoint` — HEAD unchanged after adopt.
- `adopt_refuses_detached_head_and_base_branch` — both Gate errors, exact
  strings above.
- `adopt_generic_branch_takes_the_commit_subject` — branch `wip` → goal is the
  HEAD subject.
- `adopted_first_session_continues_when_a_transcript_exists` — recording
  OneShot agent capturing `(ModelChoice, SpawnOptions)`; orchestrator
  `.with_transcript_probe(Arc::new(|_| true))`; adopt → `request_changes` →
  first spawn has `continue_session: true`; a second `request_changes` (after
  driving back to Review via `on_done`) spawns with `false`.
- `adopted_first_session_is_fresh_without_a_transcript` — probe `|_| false` →
  `continue_session: false`, and `pending_continuation` is consumed anyway.
- `native_dispatch_never_continues` — existing dispatch path records
  `continue_session: false` even with a probe of `|_| true`.
- `adopted_merge_then_abandon_flow_still_works` — adopt → `approve_merge`
  succeeds (primary on base), file lands on base.

---

## 4. Persistence (`bridge/src/store.rs`)

`PersistedTask` gains (after `comments`, before `created_at`):

```rust
/// True for a task minted around a pre-existing (user-created) worktree.
/// Defaulted so task files written before adoption existed load as native.
#[serde(default)]
pub adopted: bool,
/// Adoption's one-shot harness-continuation flag; consumed by the first
/// session spawn after adoption, persisted so a restart in between keeps it.
#[serde(default)]
pub pending_continuation: bool,
```

`persist_task` (app.rs) copies both from `ActiveTask`; `recover_task` passes both
into `reattach`.

Layer-2 tests (write first, in store.rs):

- `pre_adoption_task_files_load_as_native` — serialize `record(..)`, remove the
  two keys, load → `adopted == false`, `pending_continuation == false` (same
  fixture pattern as `pre_multi_stage_task_files_still_load_on_the_legacy_path`).
- extend `record()` + `multi_stage_record_round_trips_every_new_field`-style
  coverage: an adopted record round-trips both flags.

---

## 5. App layer (`bridge/src/app.rs`)

### 5.1 Scan cache

```rust
/// External-worktree scans are refreshed at most this often per project; the
/// board polls task.list every ~2.5 s and must never trigger a full rescan
/// per poll.
const EXTERNAL_SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// One project's cached external-worktree scan.
struct ExternalScanCache {
    scanned_at: std::time::Instant,
    worktrees: Vec<ExternalWorktree>,
}
```

`Project` gains `external_scan: Option<ExternalScanCache>` (initialized `None` in
`add_project`). `AppState` methods:

```rust
/// Canonical paths of every task-bound worktree (all states): they are Build's,
/// never external. `fs::canonicalize` with the raw path as fallback.
fn bound_worktree_paths(&self) -> std::collections::HashSet<std::path::PathBuf>

/// The project's external worktrees. Serves the cache when younger than
/// EXTERNAL_SCAN_INTERVAL; `force` bypasses the cadence (adoption-time
/// resolution). A scan error logs and returns the last-known list (or empty) —
/// task.list must stay alive. Errors are only surfaced when `force` is set.
fn external_worktrees(&mut self, project_id: &str, force: bool) -> Result<Vec<ExternalWorktree>, String>

/// Drop one project's cache so the next poll rescans (adopt/release just
/// changed what is bound).
fn invalidate_external_scan(&mut self, project_id: &str)
```

Borrow order inside `external_worktrees`: compute `bound_worktree_paths()` and
`base_for(project_id)` first, then take the project `&mut` and scan via
`crate::worktree::discover_external_worktrees(&project.repo_path, &base,
&excluded)`.

Note: a leftover bridge worktree whose task record was deleted (best-effort
cleanup failed) legitimately shows up as an external worktree — it is an orphan,
and adoption is exactly the recovery path for it.

### 5.2 Dispatch table additions

```rust
"task.adopt" => self.task_adopt(params),
"task.release" => self.task_release(params),
"worktree.diff" => self.worktree_diff(params),
```

### 5.3 `task.list` ride-along

`task_list` becomes `fn task_list(&mut self) -> Value` (the cache refresh needs
`&mut`; the dispatch arm already has it). Response:

```json
{
  "tasks": [ ...unchanged task views... ],
  "external_worktrees": [
    {
      "worktree_id": "wt-3fa9c04d21ab",
      "project_id": "proj-1",
      "project": "Build",
      "path": "/Users/ada/Projects/example/Build-hotfix",
      "branch": "hotfix/login",
      "head_sha": "abc123…full sha…",
      "head_subject": "Fix login redirect",
      "head_age_seconds": 86400,
      "dirty_files": 3,
      "diffstat": { "files_changed": 5, "insertions": 120, "deletions": 8 },
      "adoptable": true
    }
  ]
}
```

`branch` is `null` for detached HEADs. Pinned rule (computed by the handler,
which knows the base): `adoptable = branch.is_some() && branch !=
project.base_branch` — mirroring the two adopt refusals (§3.3 rules 1–2).
Entries are the scan order (§1.2.8), projects
concatenated in `self.projects` order. `task_view` gains one line:
`"adopted": active.adopted`.

### 5.4 `worktree.diff` (read-only browse)

```rust
fn worktree_diff(&mut self, params: &Value) -> Result<Value, String>
```

Params `{ project_id, worktree_id }`. Resolution rule (shared, pinned): look in
the (possibly cached) list; on a miss, force one fresh scan; still missing →
`Err(format!("unknown worktree_id: {worktree_id}"))`. Then
`crate::diff::diff_against_merge_base(&external.path, &base_for(project_id)?)`.
Response (patch/files/stat shaped exactly like `task.diff`, plus header meta so
the SPA view needs one RPC):

```json
{
  "worktree_id": "wt-3fa9c04d21ab",
  "branch": "hotfix/login",
  "head_subject": "Fix login redirect",
  "dirty_files": 3,
  "path": "/Users/ada/Projects/example/Build-hotfix",
  "adoptable": true,
  "stat": { "files_changed": 5, "insertions": 120, "deletions": 8 },
  "files": [ { "path": "src/login.rs", "status": "Modified" } ],
  "patch": "…unified diff…"
}
```

### 5.5 `task.adopt`

Params `{ project_id, worktree_id, model?, effort? }` (model/effort optional
exactly as on `task.dispatch`, via `model_choice_from`). Pinned handler shape:

```rust
fn task_adopt(&mut self, params: &Value) -> Result<Value, String> {
    let project_id = require_str(params, "project_id")?;
    let worktree_id = require_str(params, "worktree_id")?;
    let model_choice = model_choice_from(params)?;
    // Force a fresh scan: adoption must never act on a stale card (a worktree
    // adopted or removed since the last poll resolves to unknown here).
    let external = self
        .external_worktrees(&project_id, /* force */ true)?
        .into_iter()
        .find(|w| w.id == worktree_id)
        .ok_or_else(|| format!("unknown worktree_id: {worktree_id}"))?;
    let base = self.base_for(&project_id)?;
    let task_id = format!("task-{}", self.next_id);
    self.next_id += 1;
    let active = self
        .orch_for(&project_id)?
        .adopt(TaskId::new(&task_id), &external, &base, model_choice)
        .map_err(err)?;
    self.task_project.insert(task_id.clone(), project_id.clone());
    self.invalidate_external_scan(&project_id);
    let (view, persisted) = self.finish_mutation(task_id, active);
    persisted?;
    Ok(view)
}
```

Response: the task view (as `task.dispatch` returns), with `"adopted": true` and
`"state": "review"`. Note `finish_mutation` fires the Review push notification —
acceptable (throttled, and truthful: the task does need the human).

### 5.6 `task.release`

Params `{ task_id }`. Response `{ "ok": true }`. Pinned rules:

- unknown id → `"unknown task_id"`.
- `!active.adopted` →
  `"task.release: only adopted tasks can be released"`.
- terminal state →
  `format!("task.release: task is {} — use task.delete to clear it off the board", state_str(&state))`.
- Otherwise (any non-terminal state, per the release contract): delete the
  durable record **first** (`store.delete`, same ordering rationale as
  `task.delete` — if the store fails nothing changed); then `tasks.remove`,
  `active.end_session()` (a live request-changes harness must not leak),
  `task_project.remove` + `task_created_at.remove`, and
  `invalidate_external_scan(&pid)` for the removed mapping (if any). The
  worktree and branch are **never touched**.
- Deliberately requires no `project_of` resolution — a repo-missing parked
  adopted task (§5.8) has no project mapping, and release is its escape hatch.

### 5.7 `cleanup` on `task.approve_merge` and `task.git_action`

```rust
/// What happens to the worktree + branch after a user-approved merge lands.
enum MergeCleanup { Prune, Keep, Release }

/// Parse the optional `cleanup` param. Absent → Prune (today's behavior).
/// "release" is only meaningful for adopted tasks.
fn merge_cleanup_from(params: &Value, adopted: bool) -> Result<MergeCleanup, String>
```

Pinned parsing: absent/`"prune"` → `Prune`; `"keep"` → `Keep`; `"release"` →
`Release` if `adopted`, else
`Err("cleanup: \"release\" is only valid for adopted tasks".to_string())`;
anything else →
`Err(format!("invalid cleanup: {value:?} (expected prune|keep|release)"))`.

`task_approve_merge` and `task_git_action` both:

- parse cleanup up front (`task_git_action` only when `action` is `merge` or
  `merge_push`; a `cleanup` param present with `commit`/`push` →
  `Err("cleanup only applies to merge actions".to_string())`);
- keep the merge/persist ordering exactly as today; then, **after**
  `persisted?` succeeds, replace the unconditional
  `prune_merged_worktree` with:
  - `Prune` → `prune_merged_worktree(&project_id, &worktree)` (unchanged);
  - `Keep` → nothing (worktree + branch survive; the Merged record keeps the
    path bound, so it does not reappear as external until the task is deleted);
  - `Release` → delete the durable record (`store.delete(task_id)` — log, don't
    fail, the merge already landed and was acknowledged), remove the task from
    `tasks`/`task_project`/`task_created_at`, and
    `invalidate_external_scan(&project_id)`. The response is the view captured
    by `finish_mutation` (state `merged`); the board simply stops listing the
    task and the worktree resurfaces as an external card.

`task_delete` change (pinned): the best-effort worktree prune is gated —
`if worktree.path.exists() && !adopted { …discard_worktree… }` where `adopted`
is read from the task before removal. Deleting an adopted task's card never
deletes the user's files.

`task_abandon` is **unchanged** (no new params): abandon on an adopted task is a
user-triggered explicit choice ("Abandon & delete" in the split button) and
prunes worktree + branch exactly like a native abandon.

### 5.8 Boot-recovery gating (`recover_task`)

Only the **repo-missing** branch changes. Pinned replacement for the
`else if !active.task.state.is_terminal()` arm:

```rust
} else if !active.task.state.is_terminal() {
    if active.adopted {
        // Automated actions never touch (or write off) an adopted worktree:
        // park the task needs-attention instead of abandoning. A working
        // state is demoted to Interrupted (its session is gone anyway);
        // gate states (Review, Blocked, …) already need attention.
        eprintln!(
            "recover {task_id}: project repo {} is gone; parking adopted task",
            record.project_path
        );
        if active.task.state.is_working() {
            active
                .task
                .apply(TaskEvent::Interrupt)
                .map_err(|e| format!("recover {task_id}: {e}"))?;
        }
        active.last_error = Some(format!("project repo missing at {}", record.project_path));
        state_changed = true;
    } else {
        // unchanged: native task, repo gone → Abandon + last_error
    }
}
```

The worktree-missing branch stays as-is for both kinds (the worktree no longer
exists; abandoning the record is bookkeeping, not an action on user files).
A parked adopted task has no project mapping, so its actionable verbs are
`task.release` (§5.6) and `task.delete`-after-abandon is unavailable — release
is the documented way out, and the card's `last_error` says why it is parked.

### 5.9 QA scripted agent

**No new simulation functions.** Pinned facts the tests assert:

- `task.adopt` spawns no session, so QA mode needs no branch — adoption is
  synchronous bookkeeping in both modes.
- `task.request_changes` on an adopted task hits the existing
  `if self.qa_agent { self.simulate_build(...) }` branch: adopted tasks are
  Quick-kind and never multi-stage, so `simulate_build` writes `result.txt`,
  reports `done(build, completed)`, and the task returns to `Review` — exactly
  like any native Quick build session.
- `task.resume` on a parked adopted task routes through the existing
  `TaskState::Building` → `simulate_build` arm.
- The QA `Agent::Warm` never receives `SpawnOptions`; continuation is
  structurally unreachable in QA mode (and the QA `AppState` probe is
  `Arc::new(|_| false)` besides).

### 5.10 Transcript probe (real harness)

In app.rs, next to `build_agent` (it is claude-specific, like the argv):

```rust
/// Claude Code keeps one transcript directory per cwd under
/// ~/.claude/projects/, encoding the absolute path with '/' and '.' replaced
/// by '-'. Heuristic by design: a false negative just means a fresh session.
pub(crate) fn encode_claude_project_dir(path: &Path) -> String // per-char map: '/' | '.' → '-'

/// True iff the encoded directory exists under `root` and holds at least one
/// `.jsonl` transcript.
pub(crate) fn claude_transcript_exists(root: &Path, cwd: &Path) -> bool

/// The production probe, rooted at ~/.claude/projects.
fn default_claude_transcript_probe() -> crate::orchestrator::TranscriptProbe
```

`AppState` gains `transcript_probe: TranscriptProbe`, set in `new()`:
`qa_agent → Arc::new(|_| false)`, else `default_claude_transcript_probe()`.
`add_project` builds the orchestrator with
`.with_transcript_probe(self.transcript_probe.clone())`.

`build_agent`'s OneShot closure gains the third parameter and appends the flag
immediately after `--dangerously-skip-permissions`, before the model args:

```rust
Agent::OneShot(Arc::new(move |prompt: &str, choice: &ModelChoice, opts: &SpawnOptions| {
    let mut spec = HarnessSpec::new("claude")
        .arg("-p").arg(prompt)
        .arg("--mcp-config").arg(".build/mcp.json")
        .arg("--strict-mcp-config")
        .arg("--dangerously-skip-permissions");
    if opts.continue_session {
        spec = spec.arg("--continue");
    }
    for arg in choice.harness_args() { spec = spec.arg(arg); }
    spec.env("BRIDGE_MCP_SOCKET", &mcp_socket)
}))
```

### 5.11 Layer-3 tests (write first, QA mode + real repos via `init_repo`)

- `task_list_carries_external_worktrees` — add a user worktree with a dirty
  file → `task.list` has one entry, correct branch/dirty_files/diffstat/
  `adoptable: true`; a native `task.dispatch`'s worktree never appears.
- `external_scan_is_cached` — two immediate `task.list` calls run one scan
  (assert via a worktree added between the calls not appearing until the cache
  is invalidated or `EXTERNAL_SCAN_INTERVAL` passes — expose the staleness by
  calling `invalidate_external_scan` and re-listing).
- `worktree_diff_browses_without_adopting` — `worktree.diff` returns the dirty
  patch; `task.list` still shows zero tasks; unknown id → exact
  `unknown worktree_id: …` error; a raw path sent as `worktree_id` fails the
  same way.
- `adopt_mints_a_review_task_and_removes_the_card` — `task.adopt` → view has
  `state: "review"`, `adopted: true`, goal from the branch; the next
  `task.list` has no external entry for it and one task; the store record has
  `adopted: true`; double-adopt of the same id →
  `unknown worktree_id` error.
- `adopt_then_request_changes_simulates_like_any_build` — QA: request changes
  → `review` again, `result.txt` exists in the adopted worktree.
- `release_drops_the_record_and_keeps_the_files` — adopt → `task.release` →
  `{ok:true}`, task gone from `task.list`, store file gone, worktree + branch
  + checkpoint commit intact on disk, external card back on the next list
  (cache invalidated). Release on a native task / unknown / terminal → the
  three pinned errors.
- `approve_merge_cleanup_keep_keeps_the_worktree` — native task, `cleanup:
  "keep"` → merged, worktree + branch survive; default (absent) still prunes
  (both worktree and branch gone) — the backward-compat pin.
- `approve_merge_cleanup_release_unadopts_after_merge` — adopted task →
  merged into base, record gone, worktree + branch alive.
- `git_action_cleanup_rules` — `commit` with `cleanup` → error;
  `merge` + `keep` works; invalid value → pinned message; `release` on native
  → pinned message.
- `task_delete_never_prunes_an_adopted_worktree` — adopt → abandon…
  actually: adopt → merge `cleanup:"keep"` → `task.delete` → record gone,
  worktree survives. Native counterpart still prunes.
- `boot_recovery_parks_adopted_tasks_when_the_repo_is_gone` — persist an
  adopted `Review` record with a nonexistent `project_path` →
  `with_task_store` boot → state stays `review` (not abandoned), `last_error`
  set, `task.release` then works. An adopted **working**-state record parks as
  `interrupted`. A native record with the same setup still auto-abandons.
- `encode_claude_project_dir_and_probe` — `/Users/z/proj.web` →
  `-Users-z-proj-web`; probe true only when the encoded dir under a tempdir
  root holds a `.jsonl` file.

---

## 6. RPC contract summary

New methods:

| method | params | result |
|---|---|---|
| `worktree.diff` | `{ project_id, worktree_id }` | §5.4 shape |
| `task.adopt` | `{ project_id, worktree_id, model?, effort? }` | task view (`state: "review"`, `adopted: true`) |
| `task.release` | `{ task_id }` | `{ "ok": true }` |

Changed methods:

| method | change |
|---|---|
| `task.list` | adds `external_worktrees: [...]` (§5.3); task views add `adopted` |
| `task.get` / every mutation's task view | adds `"adopted": bool` |
| `task.approve_merge` | optional `cleanup: "prune"\|"keep"\|"release"`, default `"prune"` (today's behavior) |
| `task.git_action` | same optional `cleanup`, legal only with `action: "merge"\|"merge_push"` |

Unchanged: `task.abandon`, `task.delete` (params/response identical; delete's
internal prune is now gated on `!adopted`), everything else.

Pinned error strings (the SPA may key on none of them except `merge_failed:`,
which is unchanged): `unknown worktree_id: <id>` · `cannot adopt a
detached-HEAD worktree — check out a branch first` · `cannot adopt a worktree
with the base branch "<base>" checked out` · `task.release: only adopted tasks
can be released` · `task.release: task is <state> — use task.delete to clear it
off the board` · `invalid cleanup: "<value>" (expected prune|keep|release)` ·
`cleanup: "release" is only valid for adopted tasks` · `cleanup only applies to
merge actions`.

---

## 7. SPA (`spa/`)

Everything rendered from `branch`, `head_subject`, `path`, and diff content is
**untrusted** and passes through `esc()` (`core/text.js`) at every interpolation
— never through `renderMarkdown`, never unescaped into `innerHTML`. This is
already the codebase's rule; external worktrees make it load-bearing, so the new
pure markup helpers get explicit escaping tests.

### 7.1 New module `spa/src/core/splitButton.js` — the reusable split button

Generalizes the git split button currently inlined in `task.js`
`updateDiffActions`. Two exports:

```js
/** Pure markup for a GitHub-style split button. options[0] is the default.
 *  option: { id, label, menuLabel?, description, busyLabel, danger? }
 *  With one option: a plain button, no caret, no menu. All strings escaped. */
export function splitButtonMarkup(options)

/** Render into `container` and wire behavior. `run(optionId)` is awaited;
 *  while in flight the primary button is disabled and shows the option's
 *  busyLabel; rejection restores label + enabled (the caller owns error
 *  display); resolution leaves it disabled (the caller repaints/navigates).
 *  Caret toggles the menu; a pointerdown outside closes it; a menu item runs
 *  its option through the same primary button. */
export function mountSplitButton(container, { options, run })
```

Markup reuses the existing CSS classes exactly: `.splitbtn`, `.btn.primary`
(plus `.danger` when `options[0].danger`), `.caret`, `.splitmenu` (`hidden` by
default), `.mi` / `.mt` / `.md`, with `data-action="<id>"` on the primary and on
each `.mi`. The open/close/outside-click logic is lifted verbatim from today's
`#gitcaret` handler. `splitButtonMarkup` is unit-tested (node, string
assertions); `mountSplitButton` is thin wiring, exercised manually like other
view code.

### 7.2 New module `spa/src/core/adoption.js` — adopt-on-first-mutation

```js
/** A task-RPC caller for one external worktree that transparently adopts on
 *  first use. `call` is App.call-shaped (injected for tests). Adoption runs at
 *  most once; a failed adopt stays un-adopted so the next action retries. */
export function createAdoptingCall(call, projectId, worktreeId) {
  // returns {
  //   taskCall(method, params) — ensures adoption, then calls with task_id merged in,
  //   adoptedTaskId() — the task id once adopted, else null,
  // }
}
```

Unit tests (`spa/test/adoption.test.js`): first `taskCall` tasks `task.adopt`
then the method with the minted `task_id`; a second `taskCall` reuses the id
(adopt called exactly once); an adopt rejection propagates and leaves
`adoptedTaskId()` null so a retry re-adopts.

### 7.3 `spa/src/core/text.js` — age formatting

```js
/** Human-scale age: <60s "just now", <1h "Nm ago", <1d "Nh ago", else "Nd ago". */
export function humanAge(seconds)
```

Unit-tested pinned cases: `5 → "just now"`, `90 → "1m ago"`, `7200 → "2h ago"`,
`259200 → "3d ago"`.

### 7.4 New module `spa/src/core/worktreeCards.js` — external card markup

Pure (no DOM imports — testable in node):

```js
import { esc, humanAge } from "./text.js";

/** The understated board card for one external worktree (task.list entry). */
export function externalWorktreeCard(w) {
  const stat = w.diffstat || { files_changed: 0, insertions: 0, deletions: 0 };
  const dirty = w.dirty_files ? ` · ${w.dirty_files} uncommitted` : "";
  return `
    <div class="card quiet external" data-wt="${esc(w.worktree_id)}" data-project="${esc(w.project_id)}">
      <div class="top"><span class="title">${esc(w.branch || "(detached)")}</span>
        <span class="chip">WORKTREE</span></div>
      <div class="meta"><span>${esc(w.project)}</span><span>·</span><span>${esc(w.head_subject)}</span></div>
      <div class="payload">${stat.files_changed} files +${stat.insertions} −${stat.deletions}${dirty} · ${humanAge(w.head_age_seconds)}</div>
    </div>`;
}
```

Unit tests: a branch of `<img src=x onerror=alert(1)>` and a subject with
`<script>` render escaped (no raw `<` in the attribute/text positions);
detached renders `(detached)`; dirty suffix present/absent.

### 7.5 New module `spa/src/core/diffRender.js` — shared diff tables

Extract the `fileHtml` builder from `task.js` `renderDiffTab` (the
`files.map(...)` producing `.file`/`.fhead`/table rows) verbatim into:

```js
/** HTML for parsed diff files (core/diff.js parseDiff output). Escaped. */
export function diffFilesHtml(files)
```

`task.js` and the new worktree view both consume it. Unit test: a path and code
line containing `<` render escaped; hunk rows keep the `.hunk` class.

### 7.6 Router + app shell

`core/router.js`: new route both ways —

```js
case "worktree":
  if (!parts[1] || !parts[2]) return { name: "board" };
  return { name: "worktree", projectId: decodeURIComponent(parts[1]), worktreeId: decodeURIComponent(parts[2]) };
```

and in `hashFromRoute`:
`if (route.name === "worktree") return \`#/worktree/${encodeURIComponent(route.projectId)}/${encodeURIComponent(route.worktreeId)}\`;`

Router tests extend `router.test.js` (round-trip, malformed → board).
`app.js` `render()` dispatches `name === "worktree"` → `renderWorktree()` from
`views/worktree.js` (nav highlight: board).

### 7.7 Board (`views/board.js`)

`load()` keeps its single `task.list` call; `draw(res.tasks, res.external_worktrees || [])`.
After the DONE bucket, when external entries exist, render a bucket
`OTHER WORKTREES` of `externalWorktreeCard(w)` (understated: `.card.quiet.external`).
Card click → `go({ name: "worktree", projectId: c.dataset.project, worktreeId: c.dataset.wt })`
(wire via a `data-wt` selector, separate from the task-card `.card[data-id]`
wiring so task clicks are untouched). `setBadge(tasks)` keeps receiving tasks
only — external worktrees never count toward the attention badge.

CSS (`src/styles.css`): `.card.external { opacity: .68; } .card.external:hover
{ opacity: 1; } .card.external .chip { background: transparent; border: 1px
dashed var(--line); color: var(--dim); }` (match existing variable names in the
stylesheet; adjust to the tokens actually present).

### 7.8 New view `spa/src/views/worktree.js` — read-only browse + adoption

`renderWorktree()`, polling `worktree.diff { project_id, worktree_id }` every
1.6 s with the same freeze-while-commenting key discipline as the diff tab
(key = `patch`; frozen while comments exist / popover open / general box
active). Layout mirrors the task view:

- Header: `← Board`, title = branch (esc), chip `WORKTREE`, meta = subject ·
  path · "N uncommitted" (all esc'd), and a hint line: *"Read-only — acting on
  this worktree adopts it as a task."*
- Body: `diffFilesHtml(filterNoiseFiles(parseDiff(patch)))`, with the diff
  tab's comment wiring (watchSelection range flow + tap-to-comment + comment
  list + `#wgeneral` textarea) — copied from `renderDiffTab`, ids prefixed `w`.
- Action bar via `createAdoptingCall(App.call, projectId, worktreeId)`:
  - With pending comments/general text: `[Clear]` +
    `[Request Changes]` → `taskCall("task.request_changes", { comments: assembleDiffNotes(...) })`
    → on success `go({ name: "task", id: adoptedTaskId(), tab: "diff" })`.
  - Otherwise, when `adoptable`: a Merge split button (the **adopted** option
    set, §7.10) whose `run(id)` maps to
    `taskCall("task.git_action", { action, cleanup })`, plus a quiet
    `[Abandon & delete]` button (confirm: *"Delete this worktree and its
    branch? This removes files Build did not create."*) →
    `taskCall("task.abandon", {})` → `go({ name: "board" })`. Merge success:
    `go({ name: "board" })`.
  - `adoptable: false` (detached / base branch): no action bar, hint
    *"Detached HEAD — check out a branch to adopt."* (or the base-branch
    wording mirroring the bridge error).
- An RPC error containing `unknown worktree_id` paints an empty state with a
  back-to-board link (the worktree was adopted elsewhere or removed).

View code is wiring; its logic lives in the tested modules (§7.1–7.5).

### 7.9 Task view (`views/task.js`) — split-button rollout

1. **Diff-tab git actions** (`updateDiffActions`, `lastDiffState === "review"`):
   replace the inline `.splitbtn` markup + handlers with `mountSplitButton`,
   preserving today's `run` semantics (busy labels, `merge_failed:` flash via
   `mergeFailureReason`, `go(board)` after merge variants, `flash` + repaint
   after commit/push). Option sets in §7.10; `run(id)` maps to
   `App.call("task.git_action", { task_id, action, cleanup })` with `cleanup`
   omitted for `commit`/`push`.
2. **Header removal actions** (`wireActions`):
   - terminal states: the `Delete` button, unchanged;
   - live native task: `mountSplitButton` with the single option
     `abandon_delete` (renders as a plain button) — confirm text unchanged —
     → `task.abandon`;
   - live **adopted** task (`m.adopted`): options `[release (default),
     abandon_delete]`; `release` → `task.release` → `go({ name: "board" })`
     (no confirm — non-destructive); `abandon_delete` → confirm *"Delete this
     adopted worktree and its branch? This removes files Build did not create.
     The task stays as history."* → `task.abandon`. Errors hold via the
     existing `localError` banner path.
3. **Adopted marker:** in the `tmeta` row append
   `<span>·</span><span>adopted</span>` when `m.adopted` (state-only; no new
   styling required).

### 7.10 Pinned option sets

Diff-tab Merge split button — **native** task (order = menu order; first =
primary/default):

| id | label / menuLabel | description | RPC |
|---|---|---|---|
| `merge_prune` | Merge / Merge & clean up | commit, merge into `<base>`, remove the worktree + branch | `git_action merge, cleanup:"prune"` |
| `merge_keep` | — / Merge & keep worktree | merge into `<base>`, keep the worktree and branch | `git_action merge, cleanup:"keep"` |
| `merge_push` | — / Merge & push | merge, then push `<base>` to origin | `git_action merge_push, cleanup:"prune"` |
| `commit` | — / Commit | commit the work, stay on the branch | `git_action commit` |
| `push` | — / Push | commit, then push this branch to origin | `git_action push` |

**Adopted** task (and the pre-adoption worktree view, minus commit/push/
merge_push there): insert after `merge_keep`:

| id | menuLabel | description | RPC |
|---|---|---|---|
| `merge_release` | Merge & release | merge into `<base>`, then un-adopt — keep the worktree and branch, drop the task | `git_action merge, cleanup:"release"` |

(The worktree view's set is exactly `merge_prune`, `merge_keep`,
`merge_release`.)

Header split button — adopted live task:

| id | label | description | RPC |
|---|---|---|---|
| `release` | Release | un-adopt: drop the task, keep the worktree, branch, and all files | `task.release` |
| `abandon_delete` | Abandon & delete | delete the worktree and branch; the task stays as history | `task.abandon` (danger) |

Busy labels: `merging…`, `merging & pushing…`, `committing…`, `pushing…`,
`releasing…`, `abandoning…`, `requesting…`.

### 7.11 SPA tests (write first, vitest, node-pure modules only)

`spa/test/splitButton.test.js` (markup: default-first, single-option = no
caret, escaping of label/description), `spa/test/adoption.test.js` (§7.2),
`spa/test/worktreeCards.test.js` (§7.4), extend `spa/test/router.test.js`
(worktree route round-trip) and add `humanAge` cases to the text/notes test
file that already covers `core/text.js` (or a new `text.test.js`), plus
`spa/test/diffRender.test.js` (§7.5).

---

## 8. Per-layer task list (four sequential implementation agents)

Every layer: failing tests first; `cargo test && cargo clippy --all-targets --
-D warnings && cargo fmt` (bridge) / `npm test` (spa) green before each commit;
`semgrep --config auto` on changed files + `gitleaks protect --staged` before
every commit; commit as you go on `worktree-adoption`.

### Layer 1 — discovery + dirty diff (`bridge/src/worktree.rs`, `bridge/src/diff.rs`)

- Files touched: `worktree.rs` (ExternalWorktree, `Command` error variant,
  `external_worktree_id`, `discover_external_worktrees`,
  `derive_adoption_goal`, tests §1.4), `diff.rs` (`diff_against_merge_base`,
  the `diff_tree_to_dirty_workdir` refactor, tests §2.2).
- Do NOT touch: `WorktreeManager` behavior, `slugify`, `diff_against_base`'s
  observable behavior, `watch`, task.rs, orchestrator.rs, app.rs.

### Layer 2 — orchestrator adopt/continuation + store flag (`bridge/src/orchestrator.rs`, `bridge/src/store.rs`, `bridge/src/app.rs` mechanical only)

- Files touched: `orchestrator.rs` (`ActiveTask.adopted` /
  `pending_continuation`, `reattach` params, `SpawnOptions`,
  `OneShotBuilder` third param, `TranscriptProbe` + `with_transcript_probe`,
  `spawn` → `spawn_session` at all nine call sites,
  `commit_all_with_message`, `adopt`, tests §3.4), `store.rs` (two
  `#[serde(default)]` fields + tests §4), and the **mechanical** fallout in
  `app.rs`: `persist_task`/`recover_task` field plumbing (no behavior change
  yet — the §5.8 parking is Layer 3), `build_agent` closure third param with
  the `--continue` arm, `encode_claude_project_dir` /
  `claude_transcript_exists` / `default_claude_transcript_probe` +
  `AppState.transcript_probe` + `add_project` wiring, and updating existing
  test closures to the new builder arity.
- Do NOT touch: task.rs, `dispatch()`'s RPC table, task_view, QA simulate
  functions, diff.rs, the SPA.

### Layer 3 — RPC + recovery + QA coverage (`bridge/src/app.rs`)

- Files touched: `app.rs` only — `ExternalScanCache` + `Project.external_scan`
  + `EXTERNAL_SCAN_INTERVAL` + `bound_worktree_paths` / `external_worktrees` /
  `invalidate_external_scan`; `task_list` `&mut` + `external_worktrees` key;
  `task_view` `adopted`; `worktree_diff`, `task_adopt`, `task_release`;
  `MergeCleanup` + `merge_cleanup_from` + the approve_merge / git_action /
  task_delete gates; the §5.8 recover_task parking; dispatch-table entries;
  tests §5.11.
- Do NOT touch: orchestrator.rs (its API is final from Layer 2), task.rs,
  store.rs, templates, mcp.rs, the SPA. `task.abandon` semantics unchanged.

### Layer 4 — SPA (`spa/`)

- Files touched: new `src/core/splitButton.js`, `src/core/adoption.js`,
  `src/core/worktreeCards.js`, `src/core/diffRender.js`, `src/views/worktree.js`;
  edits to `src/core/text.js` (humanAge), `src/core/router.js`, `src/app.js`
  (route dispatch), `src/views/board.js` (external bucket), `src/views/task.js`
  (diff-tab split button via the component, header Release/Abandon, adopted
  marker, `diffFilesHtml` import), `src/styles.css`; tests §7.11.
- Do NOT touch: session/transport code, terminal drawer, stages view,
  notifications/settings views, the service worker, `core/notes.js` /
  `core/diff.js` parsing (consumed as-is).

---

## 9. Cross-layer invariants (assert in Layer 3's end-to-end QA test)

1. A user worktree with dirty files appears in `task.list.external_worktrees`
   within one scan interval and is browsable via `worktree.diff` without any
   task existing.
2. `task.adopt` alone: no PTY session, state `review`, `adopted: true`,
   checkpoint commit `Checkpoint: adopted by Build` present iff the tree was
   dirty and never containing `.build/mcp.json`, the external card gone from
   the very next `task.list`.
3. `task.request_changes` on the adopted task runs a normal build session
   (QA: `simulate_build`) and returns to `review`; on the real harness the
   first such session — and only the first — carries `--continue` iff a
   transcript directory exists for the worktree path.
4. `approve_merge` default prunes (native compat); `keep` preserves worktree +
   branch under a `merged` record; `release` merges, drops the record, and the
   worktree resurfaces as an external card.
5. `task.release` from any non-terminal state leaves the working directory,
   branch, and history byte-identical (modulo the earlier checkpoint commit)
   and removes the durable record.
6. Boot with a missing repo: native records auto-abandon (unchanged); adopted
   records park non-terminal with `last_error` set and remain releasable.
7. No RPC accepts a filesystem path as a worktree identifier; every branch
   name and commit subject reaching the DOM went through `esc()`.
