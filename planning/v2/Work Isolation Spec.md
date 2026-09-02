# Work Isolation — Binding Technical Spec

**Status:** Binding. Implementers follow this exactly; names and shapes below are
the contract between five sequential stages. Where this spec pins a name, use that
name. Where behavior is unspecified, match the existing code's conventions (fail
fast, pure domain core, `#[serde(default)]` forward-compat, TDD, escape everything
rendered).

**Feature summary.** Build isolates every task in its own checkout of the project.
Today that checkout is always a git linked worktree (`git worktree add`). This
feature adds a second way to materialize the same thing: a **copy-on-write clone**
of the whole project directory, `.git` included, cloned by the filesystem
(`clonefile` on APFS, reflink on btrfs/XFS) so it costs no disk and no time and
starts warm, with `node_modules`, `target` and every other ignored directory
already there. The choice is a setting on the account, overridable per project,
and locked to worktrees on any volume that cannot clone.

The word the UI and the wire use for the checkout stays **worktree**. What changes
is how a worktree is **isolated**: `worktree` (git linked worktree) or `cow`
(copy-on-write clone). "Isolation" is the one term for that choice everywhere —
type, setting, wire field, control label.

---

## 0. Global decisions (read first)

1. **One seam.** `WorktreeManager` (`bridge/src/worktree.rs`) stays the only object
   the orchestrator and the app talk to about materializing, verifying, removing
   or enumerating a checkout. It gains two backends behind the `IsolationBackend`
   trait (§2) and routes to them. **No `match Isolation` outside
   `bridge/src/isolation/`** except the two places that resolve the setting (§5)
   and the SPA controls (§7). A grep for `Isolation::Cow` outside that module at
   the end of stage 3 must find only the resolver and tests.
2. **The setting decides what NEW checkouts are; never how existing ones are
   treated.** Every existing checkout self-describes from disk (§1.2,
   `Isolation::of`). Flipping the setting orphans nothing and migrates nothing.
   Nothing about isolation is persisted on a run, a plan or an archive record.
3. **Environment is the source of truth.** No registry of clones. A clone is found
   by scanning the project's worktrees root for the marker file it carries
   (§4.6), a linked worktree by `git worktree list`, exactly as today.
4. **Refs are published, never assumed shared.** A clone is a separate repository:
   its branch tip is invisible to the project repo until `publish` copies it there
   (§2, §4.4). Every operation that reads a run branch in the project repo
   (merge, stage publication classification, branch deletion by expected head,
   restore) publishes first. For a linked worktree `publish` is a no-op, so the
   call sites are identical for both.
5. **Locked to worktrees when the volume cannot clone.** `settings.set` and
   `project.set_isolation` refuse `cow` when the probe (§6) fails, with the probe's
   reason. If the environment changes after the setting was accepted (a moved
   worktrees folder, a volume swap) a create falls back to a linked worktree and
   says so on the thread (§5.3). It never fails the dispatch.
6. **Zero behavior change until stage 4.** Stages 1 and 2 are refactors verified
   by the existing test suite; stage 3 adds the clone backend behind a probe with
   no way to select it; stage 4 adds the setting; stage 5 the controls.
7. **Absence is done.** Removal of a checkout that is already gone succeeds, for
   both backends, exactly as `WorktreeManager::remove` does today.
8. **Security.** Clients never send paths. The marker file's contents are compared,
   never used to derive a path to act on. Destination directories are always
   `worktrees_root/<name>` with `name` free of separators. `git` arguments that
   carry a name are preceded by `--`. The Issue Security Checklist stays 100/100.

---

## 1. Domain types (`bridge/src/isolation/mod.rs`)

### 1.1 `Isolation`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// A git linked worktree of the project repository (`git worktree add`).
    #[default]
    Worktree,
    /// A copy-on-write clone of the whole project directory, `.git` included.
    Cow,
}

impl Isolation {
    pub const ALL: [Isolation; 2] = [Isolation::Worktree, Isolation::Cow];
    pub fn wire(self) -> &'static str;              // "worktree" | "cow"
    pub fn from_wire(name: &str) -> Option<Self>;   // exact match only
}
```

### 1.2 `Isolation::of` — what a checkout on disk is

```rust
impl Isolation {
    /// How the checkout at `path` is isolated, read from the checkout itself.
    /// `None` when it is not a checkout Build could have made: no `.git`, or a
    /// standalone repository without the clone marker.
    pub fn of(path: &Path) -> Option<Isolation>;
}
```

- `path/.git` is a **file** → `Worktree` (git's linked-worktree pointer file).
- `path/.git` is a **directory** containing `build-isolation` (§4.6) → `Cow`.
- anything else → `None`.

No git invocation; two `stat`s. Called on every poll-path use, so it stays that cheap.

### 1.3 `IsolationAvailability`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IsolationAvailability {
    /// `Ok(())` when a clone can be made for this project on this volume;
    /// `Err(reason)` is the sentence the settings controls show.
    pub cow: Result<(), String>,
}
```

Wire shape (§5.4): `{"cow": true, "reason": null}` or `{"cow": false, "reason": "…"}`.

---

## 2. The backend trait (`bridge/src/isolation/mod.rs`)

Everything a backend does is one of these seven primitives. If an implementation
needs an eighth, the primitive is missing here, not in the caller.

```rust
pub trait IsolationBackend: Send + Sync {
    fn kind(&self) -> Isolation;

    /// Put a checkout of `branch` (which already exists in `project`) at `path`.
    /// On any failure nothing is left at `path`.
    fn materialize(
        &self,
        project: &Path,
        branch: &str,
        path: &Path,
    ) -> Result<(), WorktreeError>;

    /// The checkout at `path` is this backend's, belongs to `project`, and has
    /// `branch` checked out. Everything backend-specific about
    /// `WorktreeManager::verify_existing_worktree` lives here.
    fn verify(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError>;

    /// Make the checkout's tip of `branch` the project repo's `refs/heads/<branch>`.
    fn publish(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError>;

    /// Make the project repo's tip of `base_branch` the checkout's
    /// `refs/heads/<base_branch>`, so diffs and ahead/behind against the base
    /// mean the same thing in both isolations.
    fn sync_base(&self, project: &Path, path: &Path, base_branch: &str) -> Result<(), WorktreeError>;

    /// Delete the checkout at `path` and this backend's own record of it
    /// (`name` is git's worktree name, the directory name). Absence is success.
    fn remove(&self, project: &Path, path: &Path, name: &str) -> Result<(), WorktreeError>;

    /// Canonical paths of every checkout of `project` this backend can find
    /// under `worktrees_root` or in git's records, the primary excluded.
    fn discover(&self, project: &Path, worktrees_root: &Path) -> Result<Vec<PathBuf>, WorktreeError>;
}
```

Branch **cutting** and **deletion** are not primitives: they are operations on the
project repo and identical for both backends. They live on `WorktreeManager` (§3).

Module layout:

```
bridge/src/isolation/mod.rs        Isolation, IsolationAvailability, IsolationBackend
bridge/src/isolation/worktree.rs   WorktreeBackend  (stage 1: moved from worktree.rs)
bridge/src/isolation/cow.rs        CowBackend       (stage 3)
bridge/src/isolation/probe.rs      cow_availability (stage 3)
bridge/src/worktree.rs             Worktree, slugify, branch helpers, ExternalWorktree,
                                   describe_checkout, WorktreeManager (the façade)
```

---

## 3. The façade: `WorktreeManager` (`bridge/src/worktree.rs`)

Keeps its name and its callers. Owns `repo_path`, `worktrees_root`, and both backends.

```rust
pub struct WorktreeManager {
    repo_path: PathBuf,
    worktrees_root: PathBuf,
    worktree: WorktreeBackend,
    cow: CowBackend,                     // stage 3; stage 1 ships with `worktree` only
}

impl WorktreeManager {
    pub fn new(repo_path, worktrees_root) -> Self;

    // ---- creation: takes the resolved isolation ----------------------------
    pub fn create(&self, slug: &str, base_branch: &str, isolation: Isolation)
        -> Result<Worktree, WorktreeError>;
    pub fn create_on_branch(&self, branch: &str, base_branch: &str, isolation: Isolation)
        -> Result<NamedBranchCheckout, WorktreeError>;
    pub fn restore(&self, worktree: &Worktree, isolation: Isolation)
        -> Result<Worktree, WorktreeError>;

    // ---- everything else: dispatches on `Isolation::of(&path)` --------------
    pub fn remove(&self, worktree: &Worktree, keep_branch: bool) -> Result<(), WorktreeError>;
    pub fn remove_checkout(&self, path: &Path, name: &str) -> Result<(), WorktreeError>;
    pub fn publish(&self, path: &Path, branch: &str) -> Result<(), WorktreeError>;
    pub fn sync_base(&self, path: &Path, base_branch: &str) -> Result<(), WorktreeError>;
    pub fn merge_into_base(&self, path: &Path, branch: &str, base_branch: &str)
        -> Result<(), WorktreeError>;                       // publish, then merge in the project
    pub fn discover(&self, base_branch: &str, excluded: &HashSet<PathBuf>)
        -> Result<Vec<ExternalWorktree>, WorktreeError>;    // union of both backends
    pub fn prune(&self);                                    // best-effort stale-record cleanup

    // ---- project-repo ref operations (backend-agnostic) ---------------------
    pub fn branch_exists(&self, branch: &str) -> Result<bool, WorktreeError>;
    pub fn delete_branch_at(&self, branch: &str, expected_head: &str) -> Result<(), WorktreeError>;
    pub fn restore_branch(&self, branch: &str, sha: &str) -> Result<(), WorktreeError>;

    // ---- capability -----------------------------------------------------------
    pub fn availability(&self) -> IsolationAvailability;   // §6, computed on demand
}
```

Rules:

- `create*`/`restore` cut or find the branch in the project repo, choose the
  directory `worktrees_root/<name>` (uniqueness = branch taken **or** git worktree
  registered **or** directory exists, regardless of isolation, exactly as
  `name_taken` does today), then call `backend(isolation).materialize`.
- `restore` of a checkout that still exists calls `backend(Isolation::of(path)).verify`,
  then `publish`, then the common checks that stay in the façade: HEAD is on the
  recorded branch, HEAD equals the project's branch tip, merge-base with the base
  branch exists. The recorded branch is the truth for what to restore; the
  **isolation to recreate with is the caller's resolved setting**, never a memory
  of what the vanished checkout was.
- `remove` = `publish` when the checkout exists and `keep_branch` (a publish
  failure fails the removal: an abandon must not lose the branch), then
  `remove_checkout`, then `delete_branch_at` when `!keep_branch`. When the
  checkout is gone, `remove_checkout` still asks **every** backend to clear its
  record (a stale `git worktree` entry, nothing for a clone).
- `discover` runs both backends' `discover`, drops the primary and `excluded`,
  calls `sync_base` on each `Cow` path (best effort, logged), then
  `describe_checkout` (§4.1) on each. Sort as today.
- `merge_into_base` absorbs `Orchestrator::merge_into_base` and
  `app::merge_external_branch`, which are deleted.

---

## 4. Implementations

### 4.1 `describe_checkout` (stage 1, `bridge/src/worktree.rs`)

`parse_worktree_block` is split. The porcelain parser yields **paths only** (and
still skips bare/prunable entries silently). A new

```rust
pub fn describe_checkout(
    project: &git2::Repository,
    path: &Path,
    base_branch: &str,
    now: i64,
) -> Option<ExternalWorktree>
```

computes the summary from the checkout at `path` alone, so a clone and a linked
worktree are described by one function. `ExternalWorktree` gains
`pub isolation: Isolation` (`Isolation::of(path)`, defaulting to `Worktree` if
`None` — a foreign standalone repo never reaches here because no backend
discovers it). `external_worktrees_json` emits it as `"isolation"`.

### 4.2 `WorktreeBackend` (stage 1, moved code, no behavior change)

| primitive | body |
|---|---|
| `materialize` | `repo.worktree(name, path, opts.reference(branch_ref))` — today's `create` tail |
| `verify` | today's `verify_existing_worktree` minus the common checks kept by the façade: registered under `name`, registered path == actual, `commondir` equal |
| `publish` | `Ok(())` |
| `sync_base` | `Ok(())` |
| `remove` | today's `remove`: `remove_dir_all` if present, then prune git's record, NotFound is success |
| `discover` | `git worktree list --porcelain` paths, primary excluded |

### 4.3 Probe: `cow_availability` (stage 3, `bridge/src/isolation/probe.rs`)

```rust
pub fn cow_availability(project: &Path, worktrees_root: &Path) -> Result<(), String>
```

In order, first failure wins, each with the sentence shown to the user:

1. `project/.git` is a directory — else `"the project checkout is itself a linked worktree; clones need the repository's own .git directory"`.
2. `create_dir_all(worktrees_root)`; `metadata(project).dev() == metadata(worktrees_root).dev()` — else `"the project and the worktrees folder are on different volumes; clones cannot cross volumes"`.
3. Clone probe: write `worktrees_root/.cow-probe-<pid>` (a few bytes), clone it to `.cow-probe-<pid>.clone` with the platform call (§4.4), remove both. Failure → `"this volume does not support copy-on-write cloning ({os error})"`.
4. Any platform other than macOS and Linux → `"copy-on-write isolation is only available on macOS and Linux"`.

Cost is three `stat`s and one tiny clone; it is called from `settings.get`,
`project.list` and every create, and needs no cache.

### 4.4 The platform clone (stage 3, `bridge/src/isolation/cow.rs`)

```rust
fn clone_tree(src: &Path, dst: &Path) -> std::io::Result<()>
```

- **macOS:** one `libc::clonefile(src, dst, 0)`; APFS clones a directory recursively and atomically.
- **Linux:** walk `src`; directories are recreated (same mode), symlinks re-linked,
  regular files opened and `ioctl(dst_fd, libc::FICLONE, src_fd)`; any other file
  type is an error. Any error removes `dst` and returns it.
- The probe's file clone is the same function on a file.

`libc` is already a dependency; no new crates.

### 4.5 `CowBackend::materialize`

1. Refuse unless the project repo is `RepositoryState::Clean` and has no
   `.git/index.lock` — `"the project checkout is mid-operation; finish or abort it first"`.
2. `clone_tree(project, path)`.
3. In the clone: delete `.git/worktrees/` (inherited records of the project's
   linked worktrees, which would make git think their branches are checked out
   here), delete `.git/index.lock` if present, write the marker (§4.6).
4. In the clone: `git symbolic-ref HEAD refs/heads/<branch>`, `git reset --hard`,
   `git clean -fd` (untracked files go, ignored directories stay — that is the warm
   start).
5. Verify HEAD == the project's `refs/heads/<branch>` tip.
6. On any failure after step 2: `remove_dir_all(path)`, then return the error.

The branch was cut in the project repo by the façade before this call, so the
clone already carries it.

### 4.6 The marker: `.git/build-isolation`

Two lines: `cow\n<canonical project path>\n`. Written by `materialize`, read by
`Isolation::of` (existence only) and `discover` (path line == canonical project
path, compared as strings after canonicalizing the project). Never used to build
a path to operate on.

### 4.7 `CowBackend` other primitives

| primitive | body |
|---|---|
| `verify` | `.git` is a directory; marker present and its project line == canonical `project`; `git symbolic-ref --short HEAD` == `branch` |
| `publish` | in the project: `git fetch --no-tags --quiet -- <path> +refs/heads/<branch>:refs/heads/<branch>`. git refuses when `<branch>` is the project's checked-out branch; that refusal is the error. Bounded by the same timeout helper as `bounded_git_fetch`. |
| `sync_base` | in the clone: `git fetch --no-tags --quiet -- <project> +refs/heads/<base>:refs/heads/<base>`; same refusal if the clone has the base checked out |
| `remove` | `remove_dir_all(path)` if present; nothing else to clear |
| `discover` | `read_dir(worktrees_root)`: every directory whose `Isolation::of` is `Cow` and whose marker names this project, canonicalized |

---

## 5. Settings, resolution, wire (stage 4)

### 5.1 Persisted (bridge config JSON, `AppState::persist`)

```json
{
  "isolation": "cow",
  "projects": [ { "path": "…", "base_branch": "main", "isolation": "worktree" } ]
}
```

Top-level `isolation` absent → `worktree`. Per-project `isolation` absent → inherit.
Loaded exactly where `default_harness` and `projects` are loaded today; an unknown
value logs and is treated as absent, like `default_harness` does.

### 5.2 In-memory

- `AppState.isolation: Isolation` (account default).
- `Project.isolation: Option<Isolation>` (override).
- `AppState::resolved_isolation(&self, project_id) -> Isolation`:
  `project.isolation.unwrap_or(self.isolation)`, then `Worktree` if that is `Cow`
  and `orch.worktrees().availability().cow` is `Err`. This is one of the two
  permitted `match`/`if` sites on `Isolation` outside the module (the other is
  the setter's refusal).

### 5.3 Creation sites

Every orchestrator creation call takes the resolved isolation as an argument
(`create_bare_worktree`, `create_worktree_on_named_branch`, `dispatch_run`,
`restore_run_worktree`, and the stage-gate re-dispatch if it creates). The
orchestrator holds no isolation state. When the resolver downgraded `Cow` to
`Worktree`, the app appends a thread event (`ThreadEventKind::WorktreeCreated`'s
existing kind, or the run's `last_error`-free summary line) reading
`"Created a git worktree: copy-on-write isolation is unavailable here — <reason>"`.

### 5.4 Wire

- `settings.get` adds `"isolation": "worktree"|"cow"` and
  `"isolation_available": {"cow": bool, "reason": string|null}` (the account-level
  probe uses the first registered project, or reports `cow: false` with
  `"no project registered yet"` when there is none).
- `settings.set {"isolation": "worktree"|"cow"}` — refuses `"cow"` when unavailable:
  `"copy-on-write isolation is unavailable: <reason>; locked to worktrees"`.
  Field-wise like every other settings field; unknown value → the same shape of
  refusal `default_harness` uses.
- `project.list` rows add `"isolation": "worktree"|"cow"|null` (own override),
  `"isolation_effective": "worktree"|"cow"`, `"isolation_available": {...}`
  (this project's probe).
- New `project.set_isolation {"project_id", "isolation": "worktree"|"cow"|null}` →
  the project row. `null` clears the override. Same refusal as `settings.set` for
  an unavailable `"cow"`. Persists.

### 5.5 What does not change

`PersistedRun`, `PersistedPlan`, `PersistedArchivedWorktree`, the store schema,
`Worktree`, `ActiveRun`. The run's `worktree_path` keeps meaning "the directory".

---

## 6. Availability semantics

"Available" is per project: the project's volume and `.git` shape decide. The
account-level control reports the first project's answer because a bridge serves
one machine and one worktrees root; a mixed-volume setup surfaces per project in
`project.list`, where the per-project control shows its own reason.

---

## 7. SPA (stage 5)

- `spa/src/core/isolation.js` (pure, mirrors `core/defaultHarness.js`):
  `ISOLATIONS` naming table (`worktree` → "Git worktree", `cow` → "Copy-on-write
  clone"), `isolationOf(settings)`, `isolationLockReason(available)`,
  `isolationOptionsHtml(selected, available, {inheritLabel})`,
  `isolationPanelHtml()`, `mountIsolation(host, {callRpc})`.
- Settings page (`spa/src/views/settings.js`): a **Work isolation** panel directly
  under **Default agent**. A select with the two options; when `cow` is
  unavailable the option is disabled and the panel's hint line shows the reason and
  the words "Locked to git worktrees on this device."
- Project settings sheet (`spa/src/sheets/projectSettings.js`): a **Work
  isolation** select with three options — "Account default (Git worktree)" /
  "Git worktree" / "Copy-on-write clone" — saving on change through
  `project.set_isolation`; the same disabled-with-reason treatment.
- Copy for the hint: "A copy-on-write clone starts with the project's build
  caches already in place and keeps its own git repository. A git worktree shares
  the project's repository and starts empty."

---

## 8. Invariants a reviewer checks

1. `grep -rn "Isolation::Cow\|Isolation::Worktree" bridge/src --include=*.rs | grep -v isolation/ | grep -v "#\[test\]"` finds only `resolved_isolation`, the two setters, and `Isolation::of`'s callers passing through.
2. No `git worktree` invocation and no `find_worktree` outside `bridge/src/isolation/worktree.rs` after stage 2 (tests excepted).
3. Every place that reads `refs/heads/<run branch>` in the **project** repo is preceded by `publish` through the façade.
4. A checkout's isolation is never read from a record; only from `Isolation::of`.
5. The stage-3 test suite passes on a Linux ext4 runner (clone tests skip themselves with a printed reason; the probe test asserts the negative) and exercises the clone path on APFS.
