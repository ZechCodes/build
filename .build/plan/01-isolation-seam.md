# Stage 01 — The isolation seam: one trait, one façade, the worktree backend moved behind it

## Goal

Introduce the `IsolationBackend` trait and make `WorktreeManager` a façade that
routes to backends, with today's git-linked-worktree logic moved into the first
backend. **No behavior changes.** Every existing test passes unchanged except
where a signature gained an `Isolation` argument. After this stage a second
backend can be added without touching the orchestrator.

The binding contract is `planning/v2/Work Isolation Spec.md`. Read §0–§4.2 before
writing a line; this doc only says what to do in this stage and where the code is
today.

## Context a cold agent needs

- **Where isolation lives today.** `bridge/src/worktree.rs` holds the `Worktree`
  value type (`name`, `path`, `recorded_branch`, `base_branch`), `slugify`,
  `is_usable_branch_name`, and `WorktreeManager` with four methods: `create`,
  `create_on_branch`, `restore`, `remove`. All four call `git2`'s worktree API
  (`repo.worktree(...)`, `find_worktree`, `prune`). The same file holds
  `ExternalWorktree`, `discover_external_worktrees`, `describe_primary_checkout`,
  `describe_checkouts` (runs `git worktree list --porcelain`) and
  `parse_worktree_block`, which both parses a porcelain block and computes the
  summary for it.
- **Who calls the manager.** `bridge/src/orchestrator.rs`: `create_bare_worktree`
  (≈1410), `create_worktree_on_named_branch` (≈1423), `dispatch_run` (≈1465),
  `restore_run_worktree` (≈2656), `discard_checkout` (≈2674), `abandon_run`
  (≈2520), and `merge_into_base` (≈2931, runs `git merge` in the project repo).
  The orchestrator is constructed with `(repo_path, worktrees_root, agent,
  templates)` and builds `WorktreeManager::new(repo_path, worktrees_root)`.
- **Tests.** `cargo test` in `bridge/`; worktree tests build a temp repo with the
  `git` CLI via `tempfile`. `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt` must be clean before every commit. Run `semgrep` and `gitleaks`
  before every commit (repo rule).
- **Style.** Names explain themselves; no inline imports; fail fast; comments say
  why, not what. Doc comments in this codebase are prose paragraphs — match them.

## What to build

### 1. `bridge/src/isolation/mod.rs`

Exactly §1.1, §1.2, §1.3 and §2 of the spec: `Isolation` (with `wire`,
`from_wire`, `ALL`, `of`), `IsolationAvailability`, and the `IsolationBackend`
trait with its seven primitives. Register the module in `lib.rs`.

`Isolation::of` is two `stat`s: `.git` file → `Worktree`; `.git` directory holding
`build-isolation` → `Cow`; else `None`. Export `pub const COW_MARKER: &str =
"build-isolation"` here (the clone backend writes it in stage 3; `of` only tests
existence).

### 2. `bridge/src/isolation/worktree.rs` — `WorktreeBackend`

Move, do not rewrite. Map today's code onto the primitives per spec §4.2:

- `materialize(project, branch, path)`: open the project repo, find
  `refs/heads/<branch>`, `WorktreeAddOptions::reference`, `repo.worktree(name, path, …)`
  where `name` is `path.file_name()`.
- `verify(project, path, branch)`: the backend-specific half of
  `verify_existing_worktree` — registered under `name`, registered path
  canonicalizes to `path`, `commondir` of the checkout equals the project's.
  The common checks (HEAD on `branch`, HEAD equals the project tip, merge-base
  with base exists) stay in the façade.
- `publish`, `sync_base`: `Ok(())`.
- `remove(project, path, name)`: today's `remove` body minus the branch deletion.
- `discover(project, _root)`: `git worktree list --porcelain` → canonical paths,
  primary excluded, bare/prunable skipped. This is the path-only half of
  `describe_checkouts`.

### 3. `bridge/src/worktree.rs` — the façade

`WorktreeManager` keeps its name, `new(repo_path, worktrees_root)` signature and
private fields, plus a `worktree: WorktreeBackend` field. Implement §3 of the
spec **for one backend**: a private `fn backend(&self, isolation: Isolation) ->
&dyn IsolationBackend` that returns `&self.worktree` for `Worktree` and, for
`Cow`, an `Err(WorktreeError::Command("copy-on-write isolation is not available
in this build"))` from every entry point (stage 3 replaces that arm). A private
`fn backend_of(&self, path) -> Result<&dyn IsolationBackend, WorktreeError>` wraps
`Isolation::of`; a path that is `None` errors with the path in the message.

Public surface after this stage (signatures are binding):

```rust
pub fn create(&self, slug: &str, base_branch: &str, isolation: Isolation) -> Result<Worktree, WorktreeError>;
pub fn create_on_branch(&self, branch: &str, base_branch: &str, isolation: Isolation) -> Result<NamedBranchCheckout, WorktreeError>;
pub fn restore(&self, worktree: &Worktree, isolation: Isolation) -> Result<Worktree, WorktreeError>;
pub fn remove(&self, worktree: &Worktree, keep_branch: bool) -> Result<(), WorktreeError>;
pub fn remove_checkout(&self, path: &Path, name: &str) -> Result<(), WorktreeError>;
pub fn publish(&self, path: &Path, branch: &str) -> Result<(), WorktreeError>;
pub fn sync_base(&self, path: &Path, base_branch: &str) -> Result<(), WorktreeError>;
pub fn merge_into_base(&self, path: &Path, branch: &str, base_branch: &str) -> Result<(), WorktreeError>;
pub fn discover(&self, base_branch: &str, excluded: &HashSet<PathBuf>) -> Result<Vec<ExternalWorktree>, WorktreeError>;
pub fn prune(&self);
pub fn branch_exists(&self, branch: &str) -> Result<bool, WorktreeError>;
pub fn delete_branch_at(&self, branch: &str, expected_head: &str) -> Result<(), WorktreeError>;
pub fn restore_branch(&self, branch: &str, sha: &str) -> Result<(), WorktreeError>;
pub fn availability(&self) -> IsolationAvailability;   // this stage: cow = Err("not available in this build")
```

Rules from spec §3 that must hold now:

- `remove` publishes before removing when `keep_branch` and the checkout exists
  (a no-op for the worktree backend, but the call is there); when the checkout is
  gone, `remove_checkout` asks **every** backend to clear its record.
- `restore` of an existing checkout: `backend_of(path).verify`, then `publish`,
  then the common checks. `restore` of a missing checkout: the branch-finding
  code exactly as today (local ref, else bounded fetch from the configured
  remote), then `backend(isolation).materialize`.
- `merge_into_base` is `Orchestrator::merge_into_base` moved here verbatim (the
  "primary is on the base branch" guard, `--`, abort on failure) with a leading
  `self.publish(path, branch)?`. The orchestrator's copy is deleted and
  `run_approve_merge` calls the manager. `Orchestrator` may need
  `pub fn worktrees(&self) -> &WorktreeManager` for the app; add it.
- `discover` = union of every backend's `discover`, minus the primary and
  `excluded`, then `sync_base` best effort (logged) on each, then
  `describe_checkout` on each, then today's sort. `discover_external_worktrees`
  and `describe_primary_checkout` become thin wrappers over the manager (keep
  their signatures this stage so `app.rs` does not change; stage 2 retires them).
- `prune` is `git worktree prune` best effort via the worktree backend (moves from
  `app.rs` in stage 2; define it now).
- `branch_exists`/`delete_branch_at`/`restore_branch` are the project-repo ref
  operations (stage 2 moves `app.rs`'s `local_branch_exists`,
  `delete_local_branch_for_finish`, `restore_finish_branch_after_removal_failure`
  onto them; define them now with tests).

### 4. `describe_checkout` (spec §4.1)

Split `parse_worktree_block` so the summary is computed by
`describe_checkout(project, path, base_branch, now) -> Option<ExternalWorktree>`
from the checkout alone. Add `pub isolation: Isolation` to `ExternalWorktree`
(`Isolation::of(path).unwrap_or_default()`) and emit `"isolation"` in
`external_worktrees_json` (`app.rs`). The SPA ignores unknown fields.

### 5. Orchestrator

Thread `isolation: Isolation` through `create_bare_worktree`,
`create_worktree_on_named_branch`, `dispatch_run`, `restore_run_worktree`. The
app passes `Isolation::Worktree` at every call site this stage; stage 4 replaces
that literal with the resolver. No marker comments.

## Tests (write first)

- `isolation::tests`: `Isolation::of` on a linked worktree dir → `Worktree`; on a
  plain repo with the marker → `Cow`; on a plain repo without it → `None`; on a
  non-repo → `None`. `from_wire`/`wire` round-trip; unknown → `None`.
- `worktree::tests`: every existing test compiles with the new argument and
  passes. New: `remove` of a missing checkout still prunes git's stale record;
  `merge_into_base` refuses when the primary is not on the base; `branch_exists`,
  `delete_branch_at` (wrong expected head refuses), `restore_branch`;
  `discover` equals the old `discover_external_worktrees` output for a repo with
  two hand-made worktrees and one excluded path, and each entry carries
  `isolation == Worktree`; `describe_checkout` on a detached-HEAD worktree yields
  `branch: None`.
- `availability().cow` is `Err` in this stage.
- Orchestrator/app suites: green, unchanged in intent.

## Done when

`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` are
green; `grep -rn "find_worktree\|worktree\", \"list\|WorktreeAddOptions" bridge/src`
outside `isolation/worktree.rs` hits only tests; `Orchestrator::merge_into_base`
no longer exists; the SPA and the wire behave exactly as before.
