# Work Isolation — Primitives

Companion to the binding `planning/v2/Work Isolation Spec.md`, which absorbed every decision here; where the two
disagree the spec wins. **The rule:** two ways to materialize a checkout, one caller — everyone keeps talking to
`WorktreeManager` and the variation lives behind `IsolationBackend`, so no `match Isolation` is written outside
`bridge/src/isolation/`, save the carve-outs §0.1 pins.

## `bridge/src/isolation/mod.rs` — the types, the marker, the trait
- **`Isolation`**: the choice as a value — `ALL`, `wire`, `from_wire`, `of(&Path)`. `of` is the one authority on what
  an existing checkout is (two `stat`s, no git, no record) and never consults a setting, a record, or a caller's
  memory.
- **The marker**: `COW_MARKER` (the file name under `.git`) with `write_cow_marker(checkout, project)` and
  `cow_marker_names(checkout, project) -> bool`. §4.6's name and format are one fact with one owner, here because
  `Isolation::of` reads it in stage 1 before `cow.rs` exists; the three places that touch it — `Isolation::of`,
  `materialize`, `verify`/`discover` — spell none.
- **`IsolationAvailability`** `{ cow: Result<(), String> }` (§1.3): can a clone be made here, and if not, the sentence
  a control shows. One constructor, `of(project, worktrees_root)`, wraps `cow_availability`; a hand-written `impl
  Serialize` emits §5.4's `{"cow": bool, "reason": string|null}`. Its one behavioral method, `lock_reason(&self,
  isolation) -> Option<&str>` (`Worktree` never locked, `Cow` locked by `cow.as_ref().err()`), owns the fact that `cow`
  is the one isolation a volume can lock — the ownership that keeps the variant out of `app.rs`, where the resolver and
  the refusal ask it.
- **`IsolationBackend`**: `kind`, `materialize`, `verify`, `publish`, `sync_base`, `remove`, `discover`, `prune`,
  `holds_record`, each answering `Result<_, WorktreeError>`. Each is whole — no caller sequences two for one outcome —
  and none knows of runs, plans, threads, settings or naming. Branch cutting and deletion are absent by design:
  project-repo work, identical for both, so the façade owns them.
- **`prune(&self, project: &Path) -> Result<(), WorktreeError>`** is the eighth primitive: stale-record cleanup is a
  per-isolation variation — a linked worktree leaves a record in `.git/worktrees`, a clone leaves none — so it takes
  the shape of `publish`/`sync_base`: real work in `WorktreeBackend`, `Ok(())` in `CowBackend`. §3's "best effort" is
  the façade's policy; no backend logs or swallows.
- **`holds_record(&self, project: &Path, name: &str) -> Result<bool, WorktreeError>`** is the ninth: `prune`'s question
  asked of one name. `WorktreeBackend` answers `repo.find_worktree(name).is_ok()`, `CowBackend` `false` — a clone's
  only trace is its directory, which the façade already tests. Without it the uniqueness check would query git's
  registry itself, leaving that variation outside the trait.
- `WorktreeError` moves here (§2) — the trait's signatures are its most public use — and `worktree.rs` re-exports it,
  so `isolation/` imports nothing from the façade. It gains `NotABuildCheckout(PathBuf)`, `"not a Build checkout:
  {0}"`.

## `run_git_with_deadline` — `bridge/src/git_process.rs`
`fn run_git_with_deadline(dir: &Path, args: &[&OsStr]) -> std::io::Result<Output>`: one git child, no terminal prompt
(`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`), pipes drained, killed at a 30s deadline as `ErrorKind::TimedOut`.
`bounded_git_fetch` and the clone backend's two fetches run through it (§2).

## The two backends
**`WorktreeBackend`** (`bridge/src/isolation/worktree.rs`). Stage 1, moved code. Owns every `git worktree` invocation
and every `find_worktree` in the tree, `git worktree prune` included (§8.2) as its `prune` and the registry lookup the
façade used to make as its `holds_record`. `publish`/`sync_base` are `Ok(())` — the project repo already holds the
refs. Those no-ops, and `CowBackend`'s `Ok(())` `prune`, make both isolations one call site.

**`CowBackend`** (`bridge/src/isolation/cow.rs`). `clone_tree(src, dst)` is the one platform call (`clonefile` on
macOS, a `FICLONE` walk on Linux); the probe runs it on a file, so cloning is written once. `materialize` runs §4.5's
steps in order and unwinds the directory on any failure; `verify`/`discover` ask `cow_marker_names`.
`publish`/`sync_base` are one fetch each in opposite directions, **by path**: a clone has no configured remote, so
`configured_remote_for_branch` stays in `worktree.rs`.

## `cow_availability` — `bridge/src/isolation/probe.rs`
`fn cow_availability(project, worktrees_root) -> Result<(), String>`: four ordered checks, first failure wins, each
carrying the exact sentence the controls show. The `Err` is user copy, not a log line: the probe writes every volume
reason so neither app nor SPA writes one, and it never caches. Whether a project is registered is no fact of a volume,
so that sentence is `app.rs`'s.

## `bridge/src/worktree.rs` — the façade
**`describe_checkout(project_repo, path, base_branch, now)`** summarizes a checkout **from the checkout alone**, so a
clone and a linked worktree are one function; `parse_worktree_block` shrinks to a path-only porcelain parser feeding
the worktree backend's `discover`. "Alone" holds because `ExternalWorktree.name` is the checkout's directory basename
in both isolations, so `resolve_worktree_name`'s registry walk (worktree.rs:901) is deleted and no per-isolation name
resolution is left unnamed in the façade: Build's own checkouts have the two equal by construction and `git worktree
add <dir>` names a hand-made one after its directory; where they part — a directory renamed after registration —
`remove` finds no record, already success, and `prune` clears the stale one. That is the one behavior change stage 1
carries (spec §0.6, §4.1). `ExternalWorktree` gains `pub isolation: Isolation` from `Isolation::of(path)?` — the
function already answers an `Option`, so a path that is no Build checkout is described by nobody rather than as a
`Worktree`: one condition with `backend_of`'s one answer. `external_worktrees_json` emits it as `"isolation"` (§4.1).
**`WorktreeManager`** is the one seam, same name and callers as today, holding `repo_path`, `worktrees_root` and both
backends. `create*`/`restore` take the resolved `Isolation`; everything else reads `Isolation::of(path)`. It owns
naming and uniqueness — `name_taken` keeps its answer and its isolation-blindness but not its body. "Does some backend
hold a record of this name" is one question the façade asks twice today (worktree.rs:249 and :204's collision loop), so
one private `record_held(&self, name) -> Result<bool, WorktreeError>` owns that walk, both callers ask it, and being
fallible it makes `name_taken` fallible too. It owns branch cutting and deletion, the common `restore` checks,
publish-before-read ordering, the union `discover`, `availability()` from `IsolationAvailability::of`, and
`merge_into_base`, which absorbs `Orchestrator::merge_into_base` and `app::merge_external_branch`. No setting or record
is read here. `backend(Isolation)`, keyed on a resolved isolation, and `backend_of(&Path)`, keyed on `Isolation::of`,
are the whole of keyed dispatch — nowhere else is a `match Isolation` written or a backend field reached for. Three
primitives have nothing to key on, and each owns one `Isolation::ALL` walk no caller repeats: `record_held` walks
`holds_record` (a name carries no isolation), `remove_checkout(path, name)` (§3) walks `remove` (a gone checkout
carries none), `prune` walks `prune`. So `remove_checkout` is `record_held`'s fallible sibling and never asks
`backend_of`: absence is success for every backend's `remove`, so present and gone are one unconditional path. `prune`
alone returns nothing, being the one place turning a backend's `Err` into a log line; the record clearing `restore`
does before recreating a vanished checkout (worktree.rs:285) is that `prune`, and a record whose directory still stands
is not stale, so `materialize`'s own error is then the honest answer. `backend_of` on a path whose `Isolation::of` is
`None` is `WorktreeError::NotABuildCheckout(path)`: no git command ran, so the variant rendering "git command failed"
would name the wrong cause.

## `AppState::resolved_isolation` — `bridge/src/app.rs`
`fn resolved_isolation(&self, project_id) -> (Isolation, Option<String>)` (§5.2) puts
`project.isolation.unwrap_or(self.isolation)` to `availability.lock_reason`: `None` keeps the request, `Some(reason)`
is `(Isolation::default(), Some(reason))`. One function decides the downgrade and hands back the sentence announcing
it. The refusal `settings.set` and `project.set_isolation` share is that question asked before the setting is stored:
one private `accept_isolation` parses the wire word and is `availability.lock_reason(parsed).map_or(Ok(()), refuse)`.
Neither spells a variant — which isolation a volume can lock is `IsolationAvailability`'s fact — so
`Isolation::from_wire` is all §0.1 still carves out. `settings.get` owns the one availability sentence about a registry
rather than a volume: with no project registered it answers `cow: Err("no project registered yet")`, beside the
registry it just read (§5.4). The orchestrator holds no isolation state: creation entry points take the isolation as an
argument, and the reason, when present, joins the events a create already writes.

## `spa/src/core/isolation.js`
Mirrors `core/defaultHarness.js`: `ISOLATIONS` (the client's only naming table), `isolationOf`, `isolationLockReason`,
`isolationOptionsHtml`, `isolationPanelHtml`, `mountIsolation`, plus `ACCOUNT_ISOLATION` and `projectIsolationTarget`
(§7). Pure except `mountIsolation`, which owns the one save/refuse/repaint cycle — save, then repaint from the payload
the bridge answered with, so the control lands on what the bridge holds. It is written once because
`mountIsolation(host, {callRpc, target, settings})` takes a target — an RPC name and its fixed params — and calls
`callRpc(target.rpc, {...target.params, isolation})`, the wire word or `null` for inherit. `ACCOUNT_ISOLATION`
(`settings.set`, no params, no inherit option) serves the settings page from `settings.get`.
`projectIsolationTarget(project)` keys `project.set_isolation` on the `project.list` row's `project_id` and builds
`inheritLabel` from its `isolation_default` through `ISOLATIONS`, so §7's "Account default (Git worktree)" is composed
once from a fact the bridge owns. Neither view learns a variant name, a label, an RPC shape or a locked look.

## Boundaries and test seams
Dependencies run one way: `app`/`orchestrator` → `worktree.rs` (façade) → `isolation/` → `git_process.rs` → nothing.
`cow` tests need a cloning filesystem, so a helper in `cow.rs`'s tests returns the reason and skips aloud, the probe
test asserts that negative, and `git_process.rs` is tested on a git that never returns.
