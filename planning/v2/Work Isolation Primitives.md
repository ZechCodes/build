# Work Isolation — Primitives

Companion to the binding `planning/v2/Work Isolation Spec.md`; where the two disagree the spec wins, and
every disagreement is listed under Deviations.

**The rule.** Two ways to materialize a checkout, one caller: everyone keeps talking to `WorktreeManager`,
the variation lives behind `IsolationBackend`, and the only places naming a variant are the two impls,
`Isolation::of`, the resolver, the two setters and the SPA table.

## `bridge/src/isolation/mod.rs` — the types and the trait
- **`Isolation`**: the choice as a value — `ALL`, `wire`, `from_wire`, `of(&Path)`. `of` is the one
  authority on what an existing checkout is (two `stat`s, no git, no record) and never consults a setting,
  a record, or a caller's memory.
- **`IsolationAvailability`** `{ cow: Result<(), String> }` (§1.3): can a clone be made here, and if not,
  the sentence a control shows. Both constructors live here, so every displayable reason is written in
  this module: `of(project, worktrees_root)` wrapping `cow_availability`, and `without_project()`, whose
  reason is `"no project registered yet"`. Its hand-written `impl Serialize` emits §5.4's `{"cow": bool,
  "reason": string|null}`.
- **`IsolationBackend`**: `kind`, `materialize`, `verify`, `publish`, `sync_base`, `remove`, `discover`,
  `prune`. Each is whole — no caller sequences two for one outcome, `materialize` leaves nothing behind on
  failure — and none knows of runs, plans, threads, settings or naming. Branch cutting and deletion are
  absent by design: project-repo work, identical for both, so the façade owns them.
- **`prune(&self, project: &Path)`** is the eighth primitive, added under §2's own rule that a backend
  needing an eighth means the trait is short one. Stale-record cleanup is a per-isolation variation — a
  linked worktree leaves a record in `.git/worktrees`, a clone leaves none — so it takes the shape of
  `publish`/`sync_base`: real work in `WorktreeBackend`, `Ok(())` in `CowBackend`, best effort and logged,
  so it returns nothing.
- *Decision (spec silent):* `WorktreeError` moves here — the trait's signatures are its most public use —
  and `worktree.rs` re-exports it, so `orchestrator.rs`'s import is untouched and `isolation/` imports
  nothing from the façade.

## `run_git_with_deadline` — `bridge/src/git_process.rs`
`fn run_git_with_deadline(dir: &Path, args: &[&OsStr]) -> std::io::Result<Output>`: one git child, no
terminal prompt (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`), pipes drained, killed at a 30s
deadline as `ErrorKind::TimedOut`. §4.7's "same timeout helper as `bounded_git_fetch`" is the deadline and
the child handling, not one argv: `bounded_git_fetch` keeps its own argv and timeout sentence, so its
callers see no change, and `CowBackend` passes §4.7's argv. The module knows nothing of isolation, so
nothing circles back.

## The two backends
**`WorktreeBackend`** (`bridge/src/isolation/worktree.rs`). Stage 1, moved code. Owns every `git worktree`
invocation and every `find_worktree` in the tree, `git worktree prune` included (§8.2) as its `prune`.
`publish`/`sync_base` are `Ok(())` — the project repo already holds the refs. Those no-ops,
and `CowBackend`'s empty `prune`, are what make both isolations one call site.

**`CowBackend`** (`bridge/src/isolation/cow.rs`). `clone_tree(src, dst)` is the one platform call
(`clonefile` on macOS, a `FICLONE` walk on Linux); the probe runs it on a file, so cloning is written
once. `materialize` refuses a mid-operation project repo, clones, sanitizes the inherited `.git` (worktree
records, `index.lock`), writes the marker, sets HEAD, resets, cleans, verifies the tip and unwinds on
failure. `publish`/`sync_base` are one fetch each in opposite directions, **by path**: a clone has no
configured remote, so `configured_remote_for_branch` stays in `worktree.rs`. `prune` is `Ok(())` — no
record to go stale.

## `cow_availability` — `bridge/src/isolation/probe.rs`
`fn cow_availability(project, worktrees_root) -> Result<(), String>`: four ordered checks, first failure
wins, each carrying the exact sentence the controls show. The `Err` is user copy, not a log line — the
probe writes the volume reasons so neither the app nor the SPA writes any, and it never caches or decides
policy.

## `bridge/src/worktree.rs` — the façade
**`describe_checkout(project_repo, path, base_branch, now)`** summarizes a checkout **from the checkout
alone**, so a clone and a linked worktree are one function; `parse_worktree_block` shrinks to a path-only
porcelain parser feeding the worktree backend's `discover`.
**`WorktreeManager`** is the one seam, same name and callers as today, holding `repo_path`,
`worktrees_root` and both backends. `create*`/`restore` take the resolved `Isolation`; everything else
reads `Isolation::of(path)`. It owns naming and uniqueness (`name_taken` unchanged, isolation-blind),
branch cutting and deletion, the common `restore` checks, publish-before-read ordering, the union
`discover`, and `availability()`, computed on demand from `IsolationAvailability::of`. `merge_into_base`
moves here, absorbing `Orchestrator::merge_into_base` and `app::merge_external_branch`. No setting or
record is read here. *Decisions (spec silent):* `backend(Isolation)` and `backend_of(&Path)` are the
entire dispatch surface — nowhere else in the file is there a `match Isolation` or a concrete backend
field reached for. `prune` (§3) has no path to key on, so it takes the shape of `remove_checkout`: ask
every backend, `Isolation::ALL.map(|i| self.backend(i).prune(&self.repo_path))`, testing no variant.
`backend_of` on a path whose `Isolation::of` is `None` is a `WorktreeError::Command` naming it.

## `AppState::resolved_isolation` — `bridge/src/app.rs`
`fn resolved_isolation(&self, project_id) -> (Isolation, Option<String>)`:
`project.isolation.unwrap_or(self.isolation)`, and when that is `Cow` and the probe answers `Err`, the
pair is `(Worktree, Some(reason))`. One function decides the downgrade and hands back the sentence
announcing it, so the probe runs once per create and a creation site makes one call. It is one of the two
permitted decision sites outside the module; the other is the setter refusal, shared by `settings.set` and
`project.set_isolation` through one private `accept_isolation` holding the wire-word parse and the refusal
(*spec silent*). The orchestrator holds no isolation state: creation entry points take the isolation as an
argument, and the reason, when present, is appended where creation events are already written.

## `spa/src/core/isolation.js`
Mirrors `core/defaultHarness.js`: `ISOLATIONS` (the client's only naming table for the two words),
`isolationOf`, `isolationLockReason`, `isolationOptionsHtml`, `isolationPanelHtml`, `mountIsolation`, plus
`ACCOUNT_ISOLATION` and `projectIsolationTarget` (Deviations). Pure except `mountIsolation`, which owns
the one save/refuse/repaint cycle — save, then repaint from the payload the bridge answered with, so a
locked `cow` shows the bridge's refusal and the control lands on what the bridge holds. It is written once
because `mountIsolation(host, {callRpc, target, settings})` takes an isolation target — an RPC name and
its fixed params — and calls `callRpc(target.rpc, {...target.params, isolation})`, the wire word or `null`
for inherit. `ACCOUNT_ISOLATION` (`settings.set`, no params, no inherit option) serves the settings page
from `settings.get`; `projectIsolationTarget(projectId)` (`project.set_isolation`, the project id,
`inheritLabel: "Account default (…)"`) serves the sheet from the `project.list` row it already holds.
Neither view learns a variant name, an option label, an RPC shape or a locked option's look.

## Boundaries and test seams
Dependencies run one way: `app`/`orchestrator` → `worktree.rs` (façade) → `isolation/` → `git_process.rs`
→ nothing. `cow` tests need a cloning filesystem, so a helper in `cow.rs`'s tests returns the reason and
skips aloud, the probe test asserts that negative, and `git_process.rs` is tested on a git that never
returns.

## Deviations
- **§2's trait lists seven primitives; this doc's has eight.** `prune` joins it, on §2's own instruction
  that a needed eighth means the trait is short one; §3's `WorktreeManager::prune` keeps its signature,
  now a pass-through to every backend.
- **§5.2 pins `resolved_isolation(&self, project_id) -> Isolation`; this doc returns `(Isolation,
  Option<String>)`.** The downgrade and the sentence explaining it are one fact; a second function
  answering "was it downgraded?" decides the rule twice and re-runs the probe.
  `.build/plan/04-isolation-settings.md:44` already shapes it this way.
- **§7 pins `mountIsolation(host, {callRpc})` and a closed export list; this doc adds `target` and
  `settings` to the call and exports `ACCOUNT_ISOLATION` and `projectIsolationTarget`.** Without a target
  the save/refuse/repaint cycle is written once per view; with one it is written once.
- `.build/plan/03-cow-backend.md:22` reuses `bounded_git_fetch` itself for the cow fetches; §4.7 pins a
  different argv and borrows only its timeout, so the spec wins — `run_git_with_deadline` is the shared
  primitive, `bounded_git_fetch` one of its callers.
- §1.3's derived `Serialize` cannot emit §5.4's wire shape; the hand-written impl replaces it.
