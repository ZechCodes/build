# Stage 03 — The copy-on-write backend and the availability probe

## Goal

Implement `CowBackend` and `cow_availability` exactly per
`planning/v2/Work Isolation Spec.md` §4.3–§4.7, wire them into `WorktreeManager`
(the `Cow` arm of `backend()` and `availability()`), and prove them with tests
that run the real clone on APFS and skip themselves, with a printed reason, on a
volume that cannot clone. **Nothing selects the clone yet** — every caller still
passes `Isolation::Worktree` — so the product is unchanged after this stage.

## Context a cold agent needs

- Stage 1 created `bridge/src/isolation/{mod,worktree}.rs` and the façade; stage 2
  routed the app through it. `WorktreeManager::backend(Isolation::Cow)` currently
  errors "not available in this build" and `availability().cow` is `Err`.
- `libc` is a dependency (`bridge/Cargo.toml`). On macOS `libc::clonefile(src, dst,
  flags)` clones a file **or a whole directory tree** on APFS. On Linux
  `libc::FICLONE` is an `ioctl` on a destination fd with the source fd as the
  argument; it works per regular file on btrfs/XFS and fails with `EOPNOTSUPP` /
  `EXDEV` elsewhere. `std::os::unix::fs::MetadataExt::dev()` gives the volume id.
- The bounded fetch helper is `bounded_git_fetch` in `worktree.rs`. Spec §2's
  module layout adds `bridge/src/git_process.rs` with
  `run_git_with_deadline(dir, args) -> io::Result<Output>` (prompts disabled, pipes
  drained, 30 s deadline as `ErrorKind::TimedOut`); build it first, test it on a
  git that never returns, and route `bounded_git_fetch` and the clone backend's
  two fetches through it.
- The test machine for this repo is macOS/APFS; CI is Linux ext4. Both must be
  green.
- Repo rules: TDD, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`,
  `semgrep` + `gitleaks` before each commit, no new crates.

## What to build

### 1. `bridge/src/isolation/probe.rs`

`pub fn cow_availability(project: &Path, worktrees_root: &Path) -> Result<(), String>`
with the four checks and the four sentences of spec §4.3, in that order. Put the
platform clone call it needs in `cow.rs` (§4.4) and call it; do not duplicate.

### 2. `bridge/src/isolation/cow.rs`

- `fn clone_tree(src: &Path, dst: &Path) -> io::Result<()>` per §4.4:
  `#[cfg(target_os = "macos")]` one `clonefile`; `#[cfg(target_os = "linux")]` the
  walk with `FICLONE` per file, directories and symlinks recreated, anything else
  an error; other targets return `Unsupported`. On any error `dst` is removed
  before returning.
- `pub struct CowBackend;` implementing `IsolationBackend` per §4.5–§4.7:
  - `materialize`: the six steps of §4.5. Steps 3–5 run git in the clone with the
    same `Command` idiom the rest of the module uses (`--` before names). Failure
    after the clone removes the directory.
  - `verify`, `publish`, `sync_base`, `remove`, `discover`, `prune` per the §4.7
    table.
  - The marker: `write_cow_marker` and `cow_marker_names` from `mod.rs`; this file
    never spells the name or the format, and never opens the path in the marker.

### 3. Façade wiring (`worktree.rs`)

- Add `cow: CowBackend` to `WorktreeManager`; `backend(Isolation::Cow)` returns it.
- `availability()` = `IsolationAvailability { cow: cow_availability(&self.repo_path, &self.worktrees_root) }`.
- `remove_checkout` on a missing path already asks every backend; confirm the clone
  backend's `remove` is a no-op there.
- `restore` with `isolation == Cow` materializes a clone for the recorded branch —
  the branch must exist in the project repo, which `remove`'s publish-before-remove
  guarantees for anything Build tore down; the fetch-from-remote fallback stays.

### 4. Name uniqueness

`name_taken` already checks branch, git worktree record and directory. A clone is a
directory, so nothing changes; add the test anyway.

## Tests (write first)

A test helper `fn cow_or_skip(dir) -> bool` runs the probe on the temp dir and, on
`Err`, prints `skipping: <reason>` and returns false; every clone test starts with
`if !cow_or_skip(..) { return; }`. The probe test itself asserts: on the temp dir
of this machine the result matches `clone_tree` on a file (both succeed or both
fail — no platform assumption).

- `Isolation::of` on a materialized clone → `Cow`; on the project → `None`.
- `materialize` from a repo with an ignored `build-cache/` directory: the clone has
  it, has the branch checked out, HEAD equals the project's branch tip, has no
  `.git/worktrees`, and `git status --porcelain` is empty.
- `materialize` refuses a project mid-merge (create a conflict with the git CLI)
  and leaves nothing at the path.
- `materialize` of a project that has a linked worktree (`git worktree add` first):
  the clone does not think that worktree's branch is checked out (`git checkout
  <that branch>` inside the clone succeeds).
- `publish`: commit in the clone, publish, project's `refs/heads/<branch>` equals
  the clone's HEAD; publishing the project's checked-out branch fails.
- `sync_base`: commit on `main` in the project, sync, the clone's `refs/heads/main`
  moved; `stat_against_base` in the clone now sees the base change.
- `remove` with `keep_branch`: branch survives in the project at the clone's tip
  and the directory is gone; without: branch gone.
- `discover`: two clones and one hand-made linked worktree of the same project all
  appear, each with the right `isolation`; a clone of a **different** project
  living in the same root does not.
- `restore` of a deleted clone recreates it on the recorded branch with
  `Isolation::Cow`; `restore` of an existing clone passes `verify`; a clone whose
  marker names another project fails `verify`.
- `create(slug, base, Isolation::Cow)` end to end through the façade, and
  `create_on_branch` with an existing branch.
- `clone_tree` on a tree with a symlink and a subdirectory (Linux path) reproduces
  both; a fifo inside errors and leaves no destination.

## Done when

All of the above pass on APFS; on ext4 the clone tests print their skip reason and
the suite is green; clippy `-D warnings` and fmt clean; `grep -rn "clonefile\|FICLONE" bridge/src`
hits only `isolation/cow.rs`; `Isolation::Cow` outside `bridge/src/isolation/` is
still absent from non-test code (spec §8.1).
