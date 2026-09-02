# Work Isolation — Primitives

Companion to the binding `planning/v2/Work Isolation Spec.md`; where the two disagree the spec wins.

**The rule.** Two ways to materialize a checkout, one caller: everyone keeps talking to
`WorktreeManager`, the variation lives behind `IsolationBackend`, and the only places naming a
variant are the trait's two impls, `Isolation::of`, the resolver, the two setters and the SPA table.

## `bridge/src/isolation/mod.rs` — the types and the trait
- **`Isolation`**: the choice as a value — `ALL`, `wire`, `from_wire`, `of(&Path)`. `of` is the one
  authority on what an existing checkout is (two `stat`s, no git, no record) and never consults a
  setting, a record, or a caller's memory.
- **`IsolationAvailability`** `{ cow: Result<(), String> }` (spec §1.3): can a clone be made here,
  and if not, the sentence a control shows. Both constructors live here, so every displayable reason
  is written in this module: `of(project, worktrees_root)` wrapping `cow_availability`, and
  `without_project()`, whose reason is `"no project registered yet"` (spec §5.4). Its hand-written
  `impl Serialize` sits beside the type and emits §5.4's `{"cow": bool, "reason": string|null}`.
- **`IsolationBackend`**: `kind`, `materialize`, `verify`, `publish`, `sync_base`, `remove`,
  `discover`. Each is whole — no caller sequences two for one outcome, and `materialize` leaves
  nothing behind on failure — and none knows of runs, plans, threads, settings or naming. Branch
  cutting and deletion are absent by design: project-repo work, identical for both, so they live on
  the façade.
- *Decision (spec silent):* `WorktreeError` moves here — the trait's signatures are its most public
  use — and `worktree.rs` re-exports it, leaving `orchestrator.rs`'s import untouched. That is what
  lets `isolation/` import nothing from the façade.

## `run_git_with_deadline` — `bridge/src/git_process.rs`
`fn run_git_with_deadline(dir: &Path, args: &[&OsStr]) -> std::io::Result<Output>`: one git child,
no terminal prompt (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`), pipes drained, killed at a
30s deadline and reported as `ErrorKind::TimedOut`. This is what §4.7 means by "the same timeout
helper as `bounded_git_fetch`": the deadline and the child handling, not one argv. Its module knows
nothing of isolation, worktrees or the façade, so nothing circles back. Three callers —
`bounded_git_fetch` (`worktree.rs`), which keeps its own argv `git fetch -- <remote> <refspec>` and
its own timeout sentence so its two callers (`worktree.rs:294` restore, `app.rs:16418`
stage-publication classification) see no change, and `CowBackend`'s `publish` and `sync_base`,
passing the spec's `fetch --no-tags --quiet -- <path> +refs/heads/<b>:refs/heads/<b>`.

## The two backends
**`WorktreeBackend`** (`bridge/src/isolation/worktree.rs`). Stage 1, moved code. Owns every `git
worktree` invocation and every `find_worktree` in the tree, `git worktree prune` included (spec
§8.2) as its one inherent method beyond the seven — `prune_records(&self, project: &Path)`, best
effort and logged. `publish`/`sync_base` are `Ok(())` because the project repo already holds the
refs — that no-op is what makes both isolations one call site.

**`CowBackend`** (`bridge/src/isolation/cow.rs`). `clone_tree(src, dst)` is the one platform call
(`clonefile` on macOS, a `FICLONE` walk on Linux); the probe runs it on a file, so cloning is
written once. `materialize` refuses a mid-operation project repo, clones, sanitizes the inherited
`.git` (worktree records, `index.lock`), writes the marker, sets HEAD to the branch, resets, cleans,
verifies the tip and unwinds on failure. `publish`/`sync_base` are one `run_git_with_deadline` fetch
each in opposite directions, **by path**: a clone is no configured remote, so
`configured_remote_for_branch` stays in `worktree.rs` for restore and the orchestrator.

## `cow_availability` — `bridge/src/isolation/probe.rs`
`fn cow_availability(project, worktrees_root) -> Result<(), String>`: four ordered checks, first
failure wins, each carrying the exact sentence the controls show. The `Err` is user copy, not a log
line — that is why the probe, not the app or the SPA, writes the volume reasons; the one reason that
is not about a volume (no project to probe) belongs to `IsolationAvailability::without_project`, so
the app writes none. It never caches or decides policy.

## `bridge/src/worktree.rs` — the façade
**`describe_checkout(project_repo, path, base_branch, now)`** summarizes a checkout **from the
checkout alone**, so a clone and a linked worktree are one function; `parse_worktree_block` shrinks
to a path-only porcelain parser feeding the worktree backend's `discover`.

**`WorktreeManager`** is the one seam, same name and callers as today, holding `repo_path`,
`worktrees_root` and both backends. `create*`/`restore` take the resolved `Isolation`; everything
else reads `Isolation::of(path)`. It owns naming and uniqueness (`name_taken` unchanged,
isolation-blind), branch cutting and deletion, the common `restore` checks, publish-before-read
ordering, the union `discover`, and `availability() -> IsolationAvailability` =
`IsolationAvailability::of(&self.repo_path, &self.worktrees_root)`, computed on demand.
`merge_into_base` moves here, absorbing `Orchestrator::merge_into_base` and
`app::merge_external_branch`. No setting or persisted record is read here.
*Decisions (spec silent):* `backend(Isolation)` and `backend_of(&Path)` are the entire *trait*
dispatch surface — no `match Isolation` anywhere else in the file — but `prune` (spec §3) is not a
trait primitive and does not go through them: it calls
`self.worktree.prune_records(&self.repo_path)` on the concrete field, which is no variant test —
only one isolation keeps records. `backend_of` on a path whose `Isolation::of` is `None` is a
`WorktreeError::Command` naming the path; `remove_checkout` is the exception — absence is done, so
it asks every backend unconditionally.

## `AppState::resolved_isolation` — `bridge/src/app.rs`
`project.isolation.unwrap_or(self.isolation)`, downgraded to `Worktree` when the probe fails: one of
the two permitted decision sites outside the module, the other being the setter refusal shared by
`settings.set` and `project.set_isolation`. The orchestrator holds no isolation state — every
creation entry point takes it as an argument. With no project registered, `settings.get` answers
`IsolationAvailability::without_project()`, composing no sentence of its own. *Decisions (spec
silent):* a sibling `isolation_downgrade_note(project_id) -> Option<String>` owns the downgrade
copy, appended where creation events are already written; the two setters share one private
`accept_isolation` holding the wire-word parse and the refusal.

## `spa/src/core/isolation.js`
Mirrors `core/defaultHarness.js`: `ISOLATIONS` (the client's only naming table for the two words),
`isolationOf`, `isolationLockReason`, `isolationOptionsHtml`, `isolationPanelHtml`,
`mountIsolation`. Pure except `mountIsolation`, which owns the one save/refuse/repaint cycle — save,
then repaint from the payload the bridge answered with, so a locked `cow` shows the bridge's refusal
and the control lands on what the bridge holds.
*Decision (spec §7 pins the exports, is silent on the split):* the cycle is not written twice.
`mountIsolation(host, {callRpc, target, settings})` takes an isolation target — an RPC name and its
fixed params — and calls `callRpc(target.rpc, {...target.params, isolation})`, the wire word or
`null` for inherit. `ACCOUNT_ISOLATION = {rpc: "settings.set", params: {}, inheritLabel: null}` is
the default and serves the settings page, which paints from `settings.get`;
`projectIsolationTarget(projectId)` → `{rpc: "project.set_isolation", params: {project_id:
projectId}, inheritLabel: "Account default (…)"}` serves the sheet, which hands over the
`project.list` row it already holds as `settings`. Neither view learns a variant name, an option
label, an RPC shape or a locked option's look.

## Boundaries and test seams
Dependencies run one way: `app`/`orchestrator` → `worktree.rs` (façade) → `isolation/` →
`git_process.rs` → nothing. `cow` tests need a cloning filesystem, so a helper in
`isolation/cow.rs`'s tests returns the reason and skips aloud, the probe test asserts that negative,
and `git_process.rs` is tested on a git that never returns. Stages 1 and 2 add none.

## Deviations
- `.build/plan/03-cow-backend.md:22` reuses `bounded_git_fetch` itself for the cow fetches; §4.7
  pins a different argv there and borrows only its timeout, so the spec wins —
  `run_git_with_deadline` is the shared primitive, `bounded_git_fetch` one of its callers.
- §1.3's derived `Serialize` cannot emit §5.4's wire shape; the hand-written impl replaces it.

