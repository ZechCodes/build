# Stage 02 — Route every checkout operation in the app through the façade

## Goal

`bridge/src/app.rs` still runs its own git for finishing a worktree, classifying
stage publication, scanning for external worktrees and pruning stale records. Move
all of it onto the `WorktreeManager` façade from stage 1 so that the clone backend
in stage 3 needs **no app changes**. Behavior is unchanged; the existing app tests
prove it.

Binding contract: `planning/v2/Work Isolation Spec.md` §0.4, §3, §8.

## Context a cold agent needs

- **Stage 1 left** `WorktreeManager` with `publish`, `sync_base`,
  `merge_into_base`, `discover`, `prune`, `remove_checkout`, `branch_exists`,
  `delete_branch_at`, `restore_branch`, reachable from the app through
  `Orchestrator::worktrees()`. `app.rs` still calls the wrappers
  `discover_external_worktrees` / `describe_primary_checkout` and its own free
  functions.
- **The app's worktree-specific free functions** (all in `app.rs`, search by name):
  `run_finish_git_steps`, `remove_registered_worktree` (`git worktree remove`),
  `merge_external_branch` (`git merge` in the project), `local_branch_exists`,
  `delete_local_branch_for_finish` (`update-ref -d … expected`),
  `restore_finish_branch_after_removal_failure` (`update-ref … sha`),
  `classify_stage_publication` (reads project refs and fetches the remote),
  `checkpoint_worktree` (commits in the checkout — backend-agnostic, stays),
  `validate_finish_record_path` (path safety — stays), and the best-effort
  `git worktree prune` near `fn prune_worktrees` (≈14044).
- **Scan call sites:** `DiffCacheRefresh::ExternalScan` (≈1531),
  `external_worktrees` (≈4252), and the adoption resolver (≈16021) call
  `discover_external_worktrees(repo_path, base, excluded)`. `bound_worktree_paths`
  (≈3650) supplies `excluded` from run paths — unchanged.
- **Stage publication:** `classify_stage_publication` is called from boot recovery
  (≈2481) and `reconcile_missing_run_worktree` (≈13924) with
  `(repo_path, branch, base_branch, completion_sha)`; it reads
  `refs/heads/<branch>`-reachability in the **project** repo.
- **Finish flow:** `plan_worktree_finish` → `PersistedArchivedWorktree` (write-ahead
  record) → `run_finish_git_steps(project_path, base_branch, record)`. The record
  carries `worktree_path`, `worktree_name`, `branch`, `head_sha`.
- Tests: `cargo test` in `bridge/`; the finish tests are around
  `worktree_finish_cleanup_requires_clean_and_preserves_branch` (≈39709) and
  `run_finish_cleans_up_and_archives_the_bound_worktree` (≈31882).

## What to build

### 1. Finish steps

`run_finish_git_steps` takes `&WorktreeManager` (the project's) instead of doing
git itself:

| today | after |
|---|---|
| `remove_registered_worktree(project, path, force)` | `worktrees.remove_checkout(&path)` |
| `merge_external_branch(project, branch, base)` | `worktrees.merge_into_base(&path, branch, base)` |
| `local_branch_exists(project, branch)` | `worktrees.publish(&path, branch)` when the checkout exists, then `worktrees.branch_exists(branch)` |
| `delete_local_branch_for_finish(project, branch, head)` | `worktrees.delete_branch_at(branch, head)` |
| `restore_finish_branch_after_removal_failure` | `worktrees.restore_branch(branch, &record.head_sha)` inside the same error-composition |

A checkout is named by its directory, so `remove_checkout` takes the path alone
and no caller can hand it a name that disagrees with the path (spec §2, §4.1).
The `force` distinction disappears: `remove_checkout` deletes the directory
first, which is what `--force` bought. The Cleanup arm's "requires clean" check
already ran in planning. Delete the five free functions.

`checkpoint_worktree`, `validate_finish_record_path`, `finish_git_steps_are_complete`
stay; the last one uses `worktrees.branch_exists`.

### 1b. What stage 1 settled, so nobody unsettles it

- `WorktreeManager::remove` deletes the run branch outright when `!keep_branch`
  (a private `delete_branch`), and does **not** go through `delete_branch_at`. A
  run teardown has read no head to guard on, and inventing one would refuse
  removals that succeed today. `delete_branch_at` is for the finish path above,
  which reads a head first. Spec §3 says so; do not "fix" `remove` back onto it.
- `remove` publishes only when `Isolation::of(path)` answers — a directory that
  is no checkout has nothing to publish from, and must still be removable.
- `describe_checkout(path, base_branch, now)` takes no project repository, and
  the primary is summarized by `summarize_checkout`, which skips the
  `Isolation::of` gate the primary cannot pass.

### 2. Stage publication classification

`classify_stage_publication` becomes a method taking `&WorktreeManager`: when
`active.worktree.path` exists, call `publish(path, branch)` first (log and continue
on failure — classification must still answer). The rest of the body is unchanged.
Both call sites pass the manager from `self.orch_for(project_id)?.worktrees()`.

### 3. Scan

Replace every `discover_external_worktrees(repo, base, excluded)` with
`orch.worktrees().discover(base, excluded)`; `describe_primary_checkout` with a
manager method `describe_primary(base_branch)` (moved verbatim). Delete the two
free wrappers from `worktree.rs`. `DiffCacheRefresh::ExternalScan` currently
carries `repo_path`; it must run **without the app mutex** (see the comment at
its call site) — so give it what it needs to construct or borrow the manager off
the mutex: the simplest faithful change is to carry a `WorktreeManager` clone
(make `WorktreeManager: Clone`; it is two paths and two unit-like backends).

### 4. Prune

The best-effort `git worktree prune` in `app.rs` becomes `worktrees.prune()`.

### 5. Adoption

`adoption_params` / the adopt path builds a `Worktree` from an `ExternalWorktree`
(`name`, `path`, `branch`). Nothing changes there, but confirm no code path assumes
`.git` is a file (grep `".git"` in `app.rs`; the fs browser's `is_git` check is
about the primary and is fine).

## Tests (write first)

- The finish and run-finish suites pass unchanged.
- New: a finish `merge` of a hand-made worktree whose branch was deleted in the
  project repo between planning and stepping still refuses with today's message
  ("lost its worktree before branch deletion" family) — proves `branch_exists`
  went through the façade.
- New: `classify_stage_publication` on a real linked worktree answers identically
  before and after the change (the `publish` it now calls first is a no-op for
  that backend); the clone case is stage 3's to prove.
- Scan cache test: `external_worktrees` returns entries carrying `"isolation":
  "worktree"` on the wire.

## Done when

`cargo test`, clippy `-D warnings`, fmt clean; `grep -n "Command::new(\"git\")\|git_stdout(project_path" bridge/src/app.rs`
shows no worktree-registry, merge-into-base or branch-ref operation left in the
app (commit/rev-parse/status in the checkout are fine); spec §8 invariant 2 holds.
