# Work Isolation — Merge Brief (origin/main into build/cow)

Binding companion to `Work Isolation Spec.md` and `Work Isolation Primitives.md` for merging `origin/main` (467 commits:
Branch Selection, Bridge Concurrency) into `build/cow` (75 commits: the isolation seam, the clone backend, the setting,
the SPA controls). Where this brief and the spec disagree, the spec wins; where main's `Branch Selection Primitives.md`
or `Bridge Concurrency Primitives.md` name a component, that component keeps its name and its caller, and only the
git-touching part of its body moves behind the seam.

## 0. What each side owns after the merge
- **Structure is main's.** `app.rs`, `lifecycle.rs` (WorktreeLifecycleJob decide/run/apply), `carrier/`, `delivery.rs`,
  `reaper.rs`, `screen.rs`, `timing.rs` land as main has them. Our stage-2 routing is re-applied on top of them; it is
  not a reason to reject a main shape.
- **The seam is ours.** `bridge/src/isolation/` (the trait, both backends, the probe, the marker) lands intact, and
  `WorktreeManager` stays the one façade with both backends behind it. Spec §8 invariants hold at the end: no
  `git worktree` invocation and no `find_worktree` outside `isolation/worktree.rs`; no `Isolation::Cow|Worktree`
  outside `isolation/` except tests; every project-repo read of a run branch is preceded by `publish`.
- **The façade surface is the union.** Main's `create`, `create_on_existing_branch`, `create_cutting_branch`,
  `restore(worktree, UnregisteredRestore)`, `remove`, `remove_keeping_branch`, `discover_external_worktrees`,
  `primary_checkout_holder`, `unique_checkout_name` keep their names and callers. Main's free
  `find_primary_checkout` is the one name that does not: `WorktreeManager::describe_primary` already answers what it
  answered — the project's own checkout in the shape adoption takes for any other — and it answers it from the
  checkout alone, which is what `Work Isolation Primitives.md` names it for. The test that named the old function is
  renamed with it. The three
  creators and `restore` gain the resolved `Isolation` as their last argument (spec §5.2: the resolver answers it,
  creation sites pass it through). Our `create_on_branch` folds into main's `create_on_existing_branch`; our
  `remove_checkout(path)`, `publish`, `sync_base`, `merge_into_base`, `prune`, `branch_exists`, `delete_branch_at`,
  `restore_branch`, `availability`, `describe_primary` stay.

## 1. The branch-teardown marker, for clones
Main records `BranchTeardown` (`deletes-branch` | `keeps-branch`) in the file `build-branch-teardown` inside the
checkout's git admin directory, written at creation and read at removal and across a restore. Its writer already
resolves that directory from the checkout itself (`<checkout>/.git` as a directory, or the `gitdir:` pointer in the
`.git` file), so **writing the marker and reading it on a standing checkout are isolation-blind**: a clone's marker
lives at `<clone>/.git/build-branch-teardown`, beside `build-isolation`; a linked worktree's stays in
`<project>/.git/worktrees/<name>/`. Both markers move to `bridge/src/isolation/mod.rs` with one owner of "the
checkout's git dir" (`checkout_git_dir(path)`, main's `admin_dir_of` renamed), so no backend spells a marker path.

The one per-isolation fact is **reading the record of a checkout whose directory is gone**. That is the tenth backend
primitive: `teardown_record(&self, project: &Path, name: &str) -> Result<Option<BranchTeardown>, WorktreeError>`.
`WorktreeBackend` answers from `<commondir>/worktrees/<name>/` (`Some`, or `Ok(None)` when git holds no
registration); `CowBackend` answers `Ok(None)`, because a clone leaves no trace the project can vouch for. The façade
owns the one rule: a standing checkout is asked directly; a vanished one is asked through `teardown_record`, and
`None` means main's existing "cannot vouch" outcomes (`remove` leaves the branch; `restore` lets the caller's
`UnregisteredRestore` speak or refuse). No caller learns which backend answered.

## 2. What must be re-routed (main bypasses the seam today)
- `app.rs`: direct `git worktree prune` → `WorktreeManager::prune`; `git worktree remove` in `merge_external_branch`
  → façade `remove_checkout`/`remove`; `git worktree list --porcelain` → façade `discover`; `find_worktree` →
  `holds_record`/`teardown_record`. `merge_external_branch` itself → `WorktreeManager::merge_into_base` after
  `publish`.
- `orchestrator.rs`: `Orchestrator::merge_into_base` → `WorktreeManager::merge_into_base` (deleted in stage 2).
- `lifecycle.rs`: every job that creates or restores a checkout (create, dispatch, implement, plan workspace, adopt,
  restore) resolves isolation in `decide` (under the lock, `AppState::resolved_isolation`), carries it into `run`
  (off the lock), and says the downgrade sentence on the thread in `apply`. Pending rows carry `isolation` on the wire
  as spec §4.1 names it. `discover_external_worktrees` off the lock runs over the façade's union `discover`.
- `diff.rs` tests: use `git_fixture` and `git_process::run_git`; main's local `run_git` helper is deleted.
- SPA: the settings-page test names both "Fallback agent" (main's rename) and the "Work isolation" panel under it.

## 3. Done when
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (bridge), `npx vitest run` (spa) all
pass; the spec §8 greps find nothing; every stage doc's "Done when" greps under `.build/plan/` still hold; the
Branch Selection and Bridge Concurrency tests main added still pass unchanged in intent.
