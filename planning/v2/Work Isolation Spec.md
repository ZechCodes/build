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
   `bridge/src/isolation/`** except `accept_isolation`'s wire-word parse (§5.2)
   and the SPA controls (§7). Which isolation a volume can lock is
   `IsolationAvailability::lock_reason`'s fact (§1.3), so neither the resolver nor
   the setter names a variant: a grep for `Isolation::Cow` outside the module at
   the end of stage 4 must find only tests.
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
   by the existing test suite, with the single exception named in §4.1 (a
   checkout's `name` becomes its directory basename); stage 3 adds the clone
   backend behind a probe with no way to select it; stage 4 adds the setting;
   stage 5 the controls.
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

```rust
impl IsolationAvailability {
    /// Why `isolation` cannot be used here, or `None` when it can. `Worktree` is
    /// never locked; `Cow` is locked by the probe's reason. The one owner of
    /// which isolation a volume can lock: `resolved_isolation`'s downgrade and
    /// the setters' refusal (§5.2) are both this answer.
    pub fn lock_reason(&self, isolation: Isolation) -> Option<&str>;
}
```

Wire shape (§5.4): `{"cow": true, "reason": null}` or `{"cow": false, "reason": "…"}`,
produced by a hand-written `Serialize` (a derive cannot emit that shape). One
constructor, `IsolationAvailability::of(project, worktrees_root)`, wraps the probe.

---

## 2. The backend trait (`bridge/src/isolation/mod.rs`)

Everything a backend does is one of these nine primitives. If an implementation
needs a tenth, the primitive is missing here, not in the caller.

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

    /// Everything only this backend can check about the checkout at `path`:
    /// that it is this backend's, that it belongs to `project`, and, where the
    /// backend is the only thing that can tell, that it is on `branch`. The
    /// checks every isolation shares (HEAD on the branch, HEAD at the project's
    /// tip, a merge-base with the base) stay in `WorktreeManager`.
    fn verify(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError>;

    /// Make the checkout's tip of `branch` the project repo's `refs/heads/<branch>`.
    fn publish(&self, project: &Path, path: &Path, branch: &str) -> Result<(), WorktreeError>;

    /// Make the project repo's tip of `base_branch` the checkout's
    /// `refs/heads/<base_branch>`, so diffs and ahead/behind against the base
    /// mean the same thing in both isolations.
    fn sync_base(&self, project: &Path, path: &Path, base_branch: &str) -> Result<(), WorktreeError>;

    /// Delete the checkout at `path` and this backend's own record of it.
    /// A checkout's name is its directory basename (§4.1), so the name is the
    /// path's and no caller can pass one that disagrees. Absence is success.
    fn remove(&self, project: &Path, path: &Path) -> Result<(), WorktreeError>;

    /// Canonical paths of every checkout of `project` this backend can find
    /// under `worktrees_root` or in git's records, the primary excluded.
    fn discover(&self, project: &Path, worktrees_root: &Path) -> Result<Vec<PathBuf>, WorktreeError>;

    /// Clear this backend's stale records of checkouts that no longer exist
    /// (`git worktree prune` for linked worktrees; nothing for clones).
    fn prune(&self, project: &Path) -> Result<(), WorktreeError>;

    /// Whether this backend holds a record of a checkout called `name`
    /// (git's linked-worktree registry; a clone has no record, so `false`).
    /// The façade's uniqueness check (§3) asks this instead of querying git's
    /// registry itself, which would leave that variation outside the trait.
    fn holds_record(&self, project: &Path, name: &str) -> Result<bool, WorktreeError>;
}
```

`WorktreeError` moves into `bridge/src/isolation/mod.rs` (the trait is its most
public use) and `worktree.rs` re-exports it. It gains three variants, each for a
failure that runs no git command and so must not render as one:

- `NotABuildCheckout(PathBuf)` ("not a Build checkout: {0}") for a path whose
  `Isolation::of` is `None`.
- `IsolationUnavailable(String)` ("{0}") for an isolation this build or this
  volume cannot make, in the sentence the controls show — the answer
  `IsolationAvailability::lock_reason` gives, carried as an error.
- `Refused(String)` ("{0}") for everything the manager itself will not do: a
  branch name that is no branch name, a restore outside the managed root, a
  primary checkout on the wrong branch, a HEAD that does not match the
  persisted tip. `Command(String)` ("git command failed: {0}") is then what
  `From<GitError>` produces and nothing else — git's own words, and only when
  git spoke them.

Branch **cutting** and **deletion** are not primitives: they are operations on the
project repo and identical for both backends. They live on `WorktreeManager` (§3).

Module layout:

```
bridge/src/isolation/mod.rs        Isolation, IsolationAvailability, IsolationBackend
bridge/src/isolation/worktree.rs   WorktreeBackend  (stage 1: moved from worktree.rs)
bridge/src/isolation/cow.rs        CowBackend       (stage 3)
bridge/src/isolation/probe.rs      cow_availability (stage 3)
bridge/src/git_process.rs          run_git (stage 1), run_git_with_deadline (stage 3)
bridge/src/git_fixture.rs          init_repo, git_in (test-only: the repository every test starts from)
bridge/src/worktree.rs             Worktree, slugify, branch helpers, ExternalWorktree,
                                   describe_checkout, WorktreeManager (the façade)
```

`bridge/src/git_process.rs` is the owner of "one git child, both streams in the
failure", and it spawns a child in exactly one place.

Stage 1 introduces `run_git(dir: &Path, args: &[&str]) -> Result<String, GitError>`:
one git child in `dir`, its stdout on success, and on failure a
`GitError::Failed` carrying the command and everything git said on **both**
streams (a conflicting merge reports "CONFLICT …" on stdout, so a failure that
kept only stderr loses the reason). `GitError::Unstartable(io::Error)` is git
not starting at all. `From<GitError>` carries it into `WorktreeError` and
`OrchestratorError`, so no caller composes a git failure message of its own.
Every git child the isolation backends, the worktree façade and the
orchestrator start goes through this module; `git2` answers everything else.

Stage 3 adds `run_git_with_deadline(dir: &Path, args: &[&OsStr]) -> std::io::Result<Output>`:
the same child with terminal prompts disabled (`GIT_TERMINAL_PROMPT=0`,
`GCM_INTERACTIVE=Never`), pipes drained, killed at a 30 s deadline as
`ErrorKind::TimedOut`. `run_git` delegates to it — one child-spawning function
in the module, not two — and `bounded_git_fetch` and the clone backend's fetches
call it directly for the raw `Output` they read.

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

    // ---- everything else: keyed on `Isolation::of(&path)`, bar `remove_checkout` ----
    pub fn remove(&self, worktree: &Worktree, keep_branch: bool) -> Result<(), WorktreeError>;
    pub fn remove_checkout(&self, path: &Path) -> Result<(), WorktreeError>;  // every backend's `remove`
    pub fn publish(&self, path: &Path, branch: &str) -> Result<(), WorktreeError>;
    pub fn sync_base(&self, path: &Path, base_branch: &str) -> Result<(), WorktreeError>;
    pub fn merge_into_base(&self, path: &Path, branch: &str, base_branch: &str)
        -> Result<(), WorktreeError>;                       // publish, then merge in the project
    pub fn discover(&self, base_branch: &str, excluded: &HashSet<PathBuf>)
        -> Result<Vec<ExternalWorktree>, WorktreeError>;    // union of both backends
    pub fn prune(&self);                                    // asks every backend; the one place that logs and continues

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
  directory `worktrees_root/<name>` (uniqueness = branch taken **or** some backend
  `holds_record` under the name **or** directory exists, regardless of isolation —
  the answer `name_taken` gives today, asked of every backend instead of git's
  registry directly, which makes it `Result<bool, WorktreeError>`), then call
  `backend(isolation).materialize`. One private
  `record_held(&self, name) -> Result<bool, WorktreeError>` owns the `holds_record`
  walk over `Isolation::ALL`; `name_taken` and `create_on_branch`'s collision loop
  both ask it, and neither walks the backends itself.
- `restore` of a checkout that still exists calls `backend(Isolation::of(path)).verify`,
  then `publish`, then the common checks that stay in the façade: HEAD is on the
  recorded branch, HEAD equals the project's branch tip, merge-base with the base
  branch exists. The recorded branch is the truth for what to restore; the
  **isolation to recreate with is the caller's resolved setting**, never a memory
  of what the vanished checkout was. A vanished checkout's stale record is cleared
  by `prune` before `materialize`; a record whose directory still stands is not
  stale, and `materialize`'s own error is then the answer.
- `remove` = `publish` when there is a checkout at the path to publish from
  (`Isolation::of` answers, so a directory that is no checkout is not a reason to
  refuse to delete it) and `keep_branch` (a publish failure fails the removal: an
  abandon must not lose the branch), then `remove_checkout`, then deletes the
  branch outright when `!keep_branch` — a run teardown has no expected head to
  guard on, and inventing one would refuse removals that succeed today (§0.6).
  `delete_branch_at` is for callers that read a head first.
- `remove_checkout` asks **every** backend's `remove`, never `backend_of`: a gone
  checkout has no `Isolation::of` to key on, and absence is success for every
  backend, so present and gone are one path and a stale `git worktree` entry is
  cleared either way. It owns that `Isolation::ALL` walk as `record_held` owns the
  `holds_record` one — fallible, returned to the caller, distinct from `prune`'s
  logging walk. Those three walks are the only ones; no caller repeats them.
- `discover` runs both backends' `discover`, drops the primary and `excluded`,
  calls `sync_base` on each path (best effort, logged; a no-op for worktrees),
  then `describe_checkout` (§4.1) on each. Sort as today.
- `prune` calls every backend's `prune` and is the only place a backend's `Err`
  becomes a log line instead of a return. Backends never log or swallow.
- `merge_into_base` absorbs `Orchestrator::merge_into_base` and
  `app::merge_external_branch`, which are deleted.

---

## 4. Implementations

### 4.1 `describe_checkout` (stage 1, `bridge/src/worktree.rs`)

`parse_worktree_block` is split. The porcelain parser yields **paths only** (and
still skips bare/prunable entries silently). A new

```rust
pub fn describe_checkout(path: &Path, base_branch: &str, now: i64) -> Option<ExternalWorktree>
```

computes the summary from the checkout at `path` alone, so a clone and a linked
worktree are described by one function. It takes no project repository: every
fact comes from the checkout, so the parameter would be unused. `Isolation::of`
is the whole of the gate, which the project's own checkout cannot pass — it is
nobody's isolated copy — so the summary itself is one function down,
`summarize_checkout(path, isolation, base_branch, now)`, which `describe_checkout`
calls through the gate and `WorktreeManager::describe_primary` calls with the
default isolation. `ExternalWorktree` gains
`pub isolation: Isolation` from `Isolation::of(path)?`: the function already
answers `Option`, so a path that is not a Build checkout is described by nobody
rather than described as a `Worktree` — the same condition `backend_of` answers
with `NotABuildCheckout`. `external_worktrees_json` emits it as `"isolation"`.

`ExternalWorktree.name` is the checkout's **directory basename**, in both
isolations, and `resolve_worktree_name`'s walk of git's registry is deleted — the
last per-isolation name resolution in the façade. For every checkout Build makes
the two are the same string (`worktrees_root/<name>` is the path `restore`
insists on), and `git worktree add <dir>` names a hand-made one after its
directory. The one case they part — a linked worktree whose directory was renamed
after it was registered — is the exception to §0.6: `remove` finds no record under
the basename, which it already treats as success, and the rename left that record
stale, so `prune` clears it.

### 4.2 `WorktreeBackend` (stage 1, moved code, no behavior change)

| primitive | body |
|---|---|
| `materialize` | `repo.worktree(name, path, opts.reference(branch_ref))` — today's `create` tail |
| `verify` | today's `verify_existing_worktree` minus the common checks kept by the façade: registered under `name`, registered path == actual, `commondir` equal |
| `publish` | `Ok(())` |
| `sync_base` | `Ok(())` |
| `remove` | today's `remove`: `remove_dir_all` if present, then prune git's record, NotFound is success |
| `discover` | `git worktree list --porcelain` paths, primary excluded |
| `prune` | `git worktree prune` |
| `holds_record` | `repo.find_worktree(name).is_ok()` — the only `find_worktree` left, asked through the façade's `record_held` (§3) |

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

Two lines: `cow\n<canonical project path>\n`. The marker is one fact with one
owner in `isolation/mod.rs`: `COW_MARKER` (the file name), `write_cow_marker(checkout,
project)` and `cow_marker_names(checkout, project) -> bool`. `materialize` calls
the writer; `Isolation::of` tests existence; `verify` and `discover` ask
`cow_marker_names`. Nothing else spells the name or the format, and the marker is
never used to build a path to operate on.

### 4.7 `CowBackend` other primitives

| primitive | body |
|---|---|
| `verify` | `.git` is a directory; marker present and its project line == canonical `project`; `git symbolic-ref --short HEAD` == `branch` |
| `publish` | in the project: `git fetch --no-tags --quiet -- <path> +refs/heads/<branch>:refs/heads/<branch>`, run through `run_git_with_deadline`. git refuses when `<branch>` is the project's checked-out branch; that refusal is the error. |
| `sync_base` | in the clone: `git fetch --no-tags --quiet -- <project> +refs/heads/<base>:refs/heads/<base>`; same refusal if the clone has the base checked out |
| `remove` | `remove_dir_all(path)` if present; nothing else to clear |
| `discover` | `read_dir(worktrees_root)`: every directory whose `Isolation::of` is `Cow` and whose marker names this project, canonicalized |
| `prune` | `Ok(())` |
| `holds_record` | `Ok(false)` — a clone's only trace is its directory, which the façade already tests |

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
- `AppState::resolved_isolation(&self, project_id) -> (Isolation, Option<String>)`:
  `project.isolation.unwrap_or(self.isolation)` put to
  `orch.worktrees().availability().lock_reason(requested)` — `None` keeps the
  request, `Some(reason)` is `(Isolation::default(), Some(reason))`. The downgrade
  and the sentence announcing it are one answer.
- The setter refusal is that same question asked before the setting is stored:
  `settings.set` and `project.set_isolation` share one private
  `accept_isolation`, which parses the wire word and is
  `availability.lock_reason(parsed).map_or(Ok(()), refuse)`.
- Neither names a variant. `IsolationAvailability::lock_reason` (§1.3) owns which
  isolation a volume can lock, so `Isolation::from_wire` is the only `Isolation`
  the app spells and §0.1's carve-out covers that parse alone.

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
  `"isolation_default": "worktree"|"cow"` (the account setting the override
  replaces; the project sheet's inherit label is built from it),
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

- `spa/src/core/isolation.js` (pure except the mount, mirrors
  `core/defaultHarness.js`): `ISOLATIONS` naming table (`worktree` → "Git
  worktree", `cow` → "Copy-on-write clone"), `isolationOf(settings)`,
  `isolationLockReason(available)`, `isolationOptionsHtml(selected, available,
  {inheritLabel})`, `isolationPanelHtml()`, and one
  `mountIsolation(host, {callRpc, target, settings})` that owns the
  save/refuse/repaint cycle for both views. A target is an RPC name plus fixed
  params: `ACCOUNT_ISOLATION` (`settings.set`, no inherit option) for the
  settings page, and `projectIsolationTarget(project)` (`project.set_isolation`
  keyed on the row's `project_id`, inherit label built from the row's
  `isolation_default`) for the project sheet. Neither view learns a variant
  name, a label, an RPC shape or a locked look.
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

1. `grep -rn "Isolation::Cow\|Isolation::Worktree" bridge/src --include=*.rs | grep -v isolation/ | grep -v "#\[test\]"` finds nothing: `resolved_isolation` and `accept_isolation` go through `IsolationAvailability::lock_reason` and `Isolation::from_wire`, and every other caller passes an `Isolation` through.
2. No `git worktree` invocation and no `find_worktree` outside `bridge/src/isolation/worktree.rs` after stage 2 (tests excepted).
3. Every place that reads `refs/heads/<run branch>` in the **project** repo is preceded by `publish` through the façade.
4. A checkout's isolation is never read from a record; only from `Isolation::of`.
5. The stage-3 test suite passes on a Linux ext4 runner (clone tests skip themselves with a printed reason; the probe test asserts the negative) and exercises the clone path on APFS.
