# Work Isolation — Primitives

Companion to the binding `planning/v2/Work Isolation Spec.md`; where the two disagree the spec wins, and every
disagreement is listed under Deviations. **The rule:** two ways to materialize a checkout, one caller — everyone keeps
talking to `WorktreeManager`, the variation lives behind `IsolationBackend`, and the only places naming a variant are
the two impls, `Isolation::of`, the resolver, the two setters and the SPA table.

## `bridge/src/isolation/mod.rs` — the types, the marker, the trait
- **`Isolation`**: the choice as a value — `ALL`, `wire`, `from_wire`, `of(&Path)`. `of` is the one authority on what an
  existing checkout is (two `stat`s, no git, no record) and never consults a setting, a record, or a caller's memory.
- **The marker**: `COW_MARKER` (the file name under `.git`, `.build/plan/01-isolation-seam.md:48`) with
  `write_cow_marker(checkout, project)` and `cow_marker_names(checkout, project) -> bool`. §4.6's name and its two lines
  are one fact with one owner, here because `Isolation::of` reads it in stage 1 before `cow.rs` exists; the three places
  that touch it — `Isolation::of` (existence), `materialize` (writer), `verify`/`discover` (project line) — spell
  neither.
- **`IsolationAvailability`** `{ cow: Result<(), String> }` (§1.3): can a clone be made here, and if not, the sentence a
  control shows. One constructor, `of(project, worktrees_root)`, wrapping `cow_availability` — a checkout and a volume
  are the only facts this module owns. Its hand-written `impl Serialize` emits §5.4's
  `{"cow": bool, "reason": string|null}`.
- **`IsolationBackend`**: `kind`, `materialize`, `verify`, `publish`, `sync_base`, `remove`, `discover`, `prune`, each
  answering `Result<_, WorktreeError>`. Each is whole — no caller sequences two for one outcome, `materialize` leaves
  nothing behind on failure — and none knows of runs, plans, threads, settings or naming. Branch cutting and deletion
  are absent by design: project-repo work, identical for both, so the façade owns them.
- **`prune(&self, project: &Path) -> Result<(), WorktreeError>`** is the eighth primitive (Deviations). Stale-record
  cleanup is a per-isolation variation — a linked worktree leaves a record in `.git/worktrees`, a clone leaves none — so
  it takes the shape of `publish`/`sync_base`: real work in `WorktreeBackend`, `Ok(())` in `CowBackend`, failure
  reported like its seven siblings. §3's "best effort" is the façade's policy; no backend logs or swallows.
- *Decision (spec silent):* `WorktreeError` moves here — the trait's signatures are its most public use — and
  `worktree.rs` re-exports it, so `orchestrator.rs`'s import is untouched and `isolation/` imports nothing from the
  façade. It gains `NotABuildCheckout(PathBuf)`, `"not a Build checkout: {0}"`.

## `run_git_with_deadline` — `bridge/src/git_process.rs`
`fn run_git_with_deadline(dir: &Path, args: &[&OsStr]) -> std::io::Result<Output>`: one git child, no terminal prompt
(`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`), pipes drained, killed at a 30s deadline as `ErrorKind::TimedOut`.
§4.7's "same timeout helper as `bounded_git_fetch`" is the deadline and the child handling, not one argv (Deviations).

## The two backends
**`WorktreeBackend`** (`bridge/src/isolation/worktree.rs`). Stage 1, moved code. Owns every `git worktree` invocation
and every `find_worktree` in the tree, `git worktree prune` included (§8.2) as its `prune`. `publish`/`sync_base` are
`Ok(())` — the project repo already holds the refs. Those no-ops, and `CowBackend`'s `Ok(())` `prune` (no record to go
stale), make both isolations one call site.

**`CowBackend`** (`bridge/src/isolation/cow.rs`). `clone_tree(src, dst)` is the one platform call (`clonefile` on macOS,
a `FICLONE` walk on Linux); the probe runs it on a file, so cloning is written once. `materialize` runs §4.5 in order —
refuse a mid-operation repo, clone, sanitize the inherited `.git`, `write_cow_marker`, HEAD, reset, clean, verify the
tip, unwind on failure; `verify`/`discover` ask `cow_marker_names`. `publish`/`sync_base` are one fetch each in opposite
directions, **by path**: a clone has no configured remote, so `configured_remote_for_branch` stays in `worktree.rs`.

## `cow_availability` — `bridge/src/isolation/probe.rs`
`fn cow_availability(project, worktrees_root) -> Result<(), String>`: four ordered checks, first failure wins, each
carrying the exact sentence the controls show. The `Err` is user copy, not a log line — the probe writes every volume
reason so neither the app nor the SPA writes one, and it never caches. Whether a project is registered is no fact of a
volume, so that sentence is `app.rs`'s.

## `bridge/src/worktree.rs` — the façade
**`describe_checkout(project_repo, path, base_branch, now)`** summarizes a checkout **from the checkout alone**, so a
clone and a linked worktree are one function; `parse_worktree_block` shrinks to a path-only porcelain parser feeding the
worktree backend's `discover`. `ExternalWorktree` gains `pub isolation: Isolation` from `Isolation::of(path)`, defaulted
to `Worktree`; `external_worktrees_json` emits it as `"isolation"` (§4.1).
**`WorktreeManager`** is the one seam, same name and callers as today, holding `repo_path`, `worktrees_root` and both
backends. `create*`/`restore` take the resolved `Isolation`; everything else reads `Isolation::of(path)`. It owns naming
and uniqueness (`name_taken` unchanged, isolation-blind), branch cutting and deletion, the common `restore` checks,
publish-before-read ordering, the union `discover`, `availability()` from `IsolationAvailability::of`, and
`merge_into_base`, which absorbs `Orchestrator::merge_into_base` and `app::merge_external_branch`. No setting or record
is read here. *Decisions (spec silent):* `backend(Isolation)` and `backend_of(&Path)` are the entire dispatch surface —
nowhere else is there a `match Isolation` or a concrete backend field reached for. `pub fn prune(&self)` (§3) has no
path to key on, so it asks every backend, `Isolation::ALL`, and is the one place that turns a backend's `Err` into a log
line and carries on, which is why it alone returns nothing. `backend_of` on a path whose `Isolation::of` is `None` is
`WorktreeError::NotABuildCheckout(path)`: no git command ran, so the variant rendering "git command failed" would name
the wrong cause.

## `AppState::resolved_isolation` — `bridge/src/app.rs`
`fn resolved_isolation(&self, project_id) -> (Isolation, Option<String>)` is
`project.isolation.unwrap_or(self.isolation)`, and `(Worktree, Some(reason))` when that is `Cow` and the probe answers
`Err`. One function decides the downgrade and hands back the sentence announcing it, so the probe runs once per create
and a creation site makes one call. It is one of the two permitted decision sites outside the module; the other is the
setter refusal, shared by `settings.set` and `project.set_isolation` through one private `accept_isolation` holding the
wire-word parse and the refusal (*spec silent*). `settings.get` owns the one availability sentence about the registry
rather than a volume: with no project registered it answers
`IsolationAvailability { cow: Err("no project registered yet") }`, beside the registry it just read (§5.4). The
orchestrator holds no isolation state: creation entry points take the isolation as an argument, and the reason, when
present, joins the events a create already writes.

## `spa/src/core/isolation.js`
Mirrors `core/defaultHarness.js`: `ISOLATIONS` (the client's only naming table for the two words), `isolationOf`,
`isolationLockReason`, `isolationOptionsHtml`, `isolationPanelHtml`, `mountIsolation`, plus `ACCOUNT_ISOLATION` and
`projectIsolationTarget` (Deviations). Pure except `mountIsolation`, which owns the one save/refuse/repaint cycle —
save, then repaint from the payload the bridge answered with, so a locked `cow` shows the bridge's refusal and the
control lands on what the bridge holds. It is written once because `mountIsolation(host, {callRpc, target, settings})`
takes an isolation target — an RPC name and its fixed params — and calls
`callRpc(target.rpc, {...target.params, isolation})`, the wire word or `null` for inherit. `ACCOUNT_ISOLATION`
(`settings.set`, no params, no inherit option) serves the settings page from `settings.get`.
`projectIsolationTarget(project)` takes the `project.list` row the sheet already holds, keys `project.set_isolation` on
its `project_id`, and builds `inheritLabel` from the row's `isolation_default` — the account default an override
replaced — through `ISOLATIONS`, so §7's "Account default (Git worktree)" is composed once from a fact the bridge owns.
Neither view learns a variant name, a label, an RPC shape or a locked look.

## Boundaries and test seams
Dependencies run one way: `app`/`orchestrator` → `worktree.rs` (façade) → `isolation/` → `git_process.rs` → nothing.
`cow` tests need a cloning filesystem, so a helper in `cow.rs`'s tests returns the reason and skips aloud, the probe
test asserts that negative, and `git_process.rs` is tested on a git that never returns.

## Deviations
- **§2's trait lists seven primitives; this doc's has eight.** `prune` joins it, on §2's own instruction that a needed
  eighth means the trait is short one; §3's `WorktreeManager::prune` keeps its signature, now a pass-through to every
  backend and the only place that logs and continues.
- **§5.2 pins `resolved_isolation(&self, project_id) -> Isolation`; this doc returns `(Isolation, Option<String>)`.**
  The downgrade and the sentence explaining it are one fact; a second function answering "was it downgraded?" decides
  the rule twice (`.build/plan/04-isolation-settings.md:44` agrees).
- **§5.4's `project.list` row carries `isolation`, `isolation_effective`, `isolation_available`; this doc adds
  `isolation_default`.** §7's inherit label names the account default, which no other field answers once an override is
  set. `project_json` already reads `AppState.isolation` for `isolation_effective`, so this emits a fact it holds
  instead of making the sheet fetch settings.
- **§7 pins `mountIsolation(host, {callRpc})` and a closed export list; this doc adds `target` and `settings` to the
  call and exports `ACCOUNT_ISOLATION` and `projectIsolationTarget`.** Without a target the save/refuse/repaint cycle is
  written once per view; with one it is written once.
- `.build/plan/03-cow-backend.md:22` reuses `bounded_git_fetch` itself for the cow fetches; §4.7 pins a different argv
  and borrows only its timeout, so the spec wins — `run_git_with_deadline` is the shared primitive.
- §1.3's derived `Serialize` cannot emit §5.4's wire shape; the hand-written impl replaces it.
