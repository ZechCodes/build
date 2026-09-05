# Branch selection — primitives

## Boundaries

| Module | Owns | Hides | Knows nothing of |
|---|---|---|---|
| `bridge/src/gitgui.rs` | git facts about refs, including **where a branch's ref lives** | git2 iteration, remote choice, symbolic-ref exclusion, ahead/behind, diffstat | runs, worktrees, adoption |
| `bridge/src/worktree.rs` | materialising a checkout for a named branch, and what teardown may take with it | fetch, tracking-ref creation, directory naming, the on-disk teardown marker and its survival across restore | JSON, RPC, runs |
| `bridge/src/app.rs` | which action a branch resolves to, and doing it | runs, external scan, projects, deferral | presentation |
| `spa/src/core/branchPickerModel.js` | ranking + row copy (pure) | fuzzy scoring, the cut-new row | DOM, network |
| `spa/src/core/createWork.js` | DOM + navigation | — | classification |

`gitgui.rs` loses a dependency and keeps its subject: `branch_entry_json`'s `external_worktree_id` stamp — a worktree/adoption concept inside the git-facts module — moves out to `app.rs` (§a), and no run, adoption or intent concept follows it in.

One new edge goes the other way: `worktree.rs` calls `gitgui::branch_origin` (§c). "Which ref backs this branch name" is a git fact about refs, so it lives in the git-facts module and has exactly one definition.

## (a) The project-scoped branch listing

Lives where it already does: `git.branches` → `AppState::git_branches` → `gitgui::branch_list`, via `defer_branch_listing`.

**gitgui emits git facts only, typed.** `branch_list` sheds `external_branches`, gains remote-tracking refs that have no local counterpart so the picker sees `origin/feature-x`, and hands back typed rows instead of finished JSON:

```rust
pub struct BranchRow {
    pub name: String,
    pub is_current: bool,
    pub origin: BranchOrigin,        // Local | Remote { remote, tracking_ref }
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub head_subject: String,
    pub head_time: i64,
    pub stat: crate::diff::Stat,
}
pub struct BranchListing { pub current: Option<String>, pub rows: Vec<BranchRow> }
pub fn branch_list(repo_path: &Path, base_branch: &str) -> Result<BranchListing, String>
```

The row **carries** its origin rather than describing it twice, and nothing downstream re-derives it.

**How the remote rows are found, and which refs are not branches.** `branch_list` keeps its local iteration and adds one pass per remote: for each name in `repo.remotes()`, every reference under `refs/remotes/<remote>/`, the branch name being the suffix after that prefix. Two refs in that pass are skipped, and both skips are pinned:

- A reference whose `kind()` is `Symbolic` is not a branch and is never a row (`origin/HEAD` in every clone with a remote). The exclusion is by reference kind, not by the spelling `HEAD`.
- A remote-tracking ref whose suffix already exists as `refs/heads/<name>` is the local branch's upstream, not a second branch.

`branch_origin` (§c) applies the same kind rule to its remote candidates, so the listing and the start agree on what counts as a remote branch.

**Every branch git has is a live row.** The listing does not filter, flag or relabel by name. The `is_usable_branch_name` narrowing guards only names Build is about to **cut**, so no listed row is refused on click.

**The app layer stamps ownership.** `ProjectCheckouts` (the on-lock half) carries the primary checkout's repo path, the external scan's branch → worktree-id map, and every live run's checkout; `ProjectCheckouts::holders` turns that into the `BranchOwnershipIndex` the rows are stamped from. `BranchHolder` picks the one holder that speaks for a branch, so a row names exactly one thing to press.

Wire row:

```json
{ "name":"feature-x", "remote":"origin", "is_current":false,
  "upstream":"origin/feature-x", "ahead":0, "behind":3,
  "head_subject":"…", "head_time":1756900000,
  "stat":{"files_changed":4,"insertions":80,"deletions":9},
  "external_worktree_id":null, "run_id":null, "primary_worktree_id":null }
```

## (b) One polymorphic resolution, owned by the bridge

Facts go in, a strategy comes out, and the strategy names its own label — so the listing's verb and the start's execution are the same object, not two matches on the same variation.

**A name is not the same input as words.** Two callers reach this decision with an absent ref and want opposite things, and no fact about the repository can tell them apart, so the difference is carried in as an input — here, as which of two RPC slots the caller filled:

```
worktree.create { project_id, branch } -> a branch that already exists, here or on a remote
worktree.create { project_id, name }   -> words to cut a new branch after
```

Exactly one of `branch` / `name` is accepted; both or neither is an error. The `BranchTarget` type the table below is written against belongs to the later `branch.start` item; here the distinction is carried by the two params and the two `WorktreeManager` verbs of §(c).

- `branch.dispatch` with `branch: "hotfix-login"` must cut `hotfix-login` exactly, and `build/csv-export` must not become `build/build-csv-export`.
- The picker's typed-text row must preview and produce `build/hotfix-login`.

**Two name rules, each with one home.**

- `worktree::is_ref_name(name)` — git's own branch-name rule: `git2::Reference::is_valid_name("refs/heads/<name>")` plus the two narrowings `git check-ref-format --branch` makes (no leading `-`, not exactly `HEAD`). `worktree.create { branch }` validates with this and refuses outright if it fails — never silently demoted to words.
- `worktree::is_usable_branch_name(name)` — Build's narrowing to `[A-Za-z0-9._-]` segments. It guards names Build is about to **cut**, and only those.

`branch.dispatch` keeps its single free-text slot and adapts it at its own edge, with the rule `cut_branch_for_dispatch` uses today: a `branch` that passes `is_usable_branch_name` is a name to cut exactly, anything else is words to slugify.

**The single match is exhaustive by construction.** The three ownership lookups come first for both targets; only then does the target decide. Both callers ask them the same way: `worktree.create {branch}` and `branch.dispatch {branch}` each build a `BranchHolder` from `ProjectCheckouts::holders()` over a forced rescan, so the two verbs never disagree about who has a branch. Two survivors of that resolution are named here so nobody looks for what does not exist: `branch.dispatch` opens the run and adopts the external worktree the holder names, but a branch the **primary** checkout holds is *refused* by dispatch (with the same structured message `worktree.create {branch}` gives) rather than adopted — adopting the primary from a dispatch is deferred to the `branch.start` item, which owns the `AdoptCheckout { primary: true }` strategy; and dispatch keeps `cut_branch_for_dispatch` as its own adapter for a slot that may be a name or words.

| facts | strategy | intent |
|---|---|---|
| `run_id: Some` (either target) | open the run | `Open` |
| `external_worktree_id: Some` (either target) | adopt that checkout | `Adopt` |
| `primary_worktree_id: Some` (either target) | adopt the primary | `Adopt` |
| `target: Ref { origin: Local }` | check the branch out | `Checkout` |
| `target: Ref { origin: Remote }` | fetch, then check out | `Materialise` |
| ~~`target: Ref { origin: Absent }`~~ | ~~cut it exactly as given~~ | ~~`Cut`~~ |
| `target: Words` | cut `build/<slug>` | `Cut` |

**The `Absent` row is superseded** by the two-verb split in §(c): a caller that spelled a `branch` is refused, and only `branch.dispatch`'s own verb cuts an absent name. The row is kept struck through here because the `branch.start` table it belongs to is the later item's, and that item inherits the refusal rather than the cut.

**What `Words` does when the ref exists.** Nothing different: once no run, external worktree or primary checkout claims the raw text, `Words` short-circuits to the cut regardless of whether a ref of that spelling exists. That is what makes the "typed-name behaviour stays byte-identical" claim true.

## (c) Remote-branch materialisation, and whose branch it is

The ref-layout fact lives in `gitgui.rs`, and `worktree.rs` consumes it:

```rust
pub enum BranchOrigin { Local, Remote { remote: String, tracking_ref: String }, Absent }
pub fn branch_origin(repo: &Repository, branch: &str) -> Result<BranchOrigin, git2::Error>
```

**`branch_origin` has an error channel, and only one git2 answer means `Absent`.** `NotFound` for `refs/heads/<branch>` and for every candidate `refs/remotes/<remote>/<branch>` is `Absent`. Every other error — `InvalidSpec` included — is `Err`, because every caller has already guaranteed the name is a branch name.

**Only a direct remote-tracking ref is a candidate.** A candidate that is found but whose `kind()` is `Symbolic` counts as not found for that remote — the same rule `branch_list` applies to its rows.

**Which remote wins.** `origin` first if it has a direct ref of that name, otherwise the first remote in `repo.remotes()` order that does. A branch with a local ref is `Local` however many remotes also carry it.

### Two verbs, not a mode flag

The two callers differ on **which verb they want**, not on a mode of one verb, so `worktree.rs` exposes two:

```rust
pub fn create_on_existing_branch(&self, branch: &str, base_branch: &str)
    -> Result<NamedBranchCheckout, WorktreeError>
pub fn create_cutting_branch(&self, branch: &str, base_branch: &str)
    -> Result<NamedBranchCheckout, WorktreeError>
```

- `create_on_existing_branch` — `Local` checks out; `Remote` materialises the local tracking branch first; `Absent` is `Err("branch {branch:?} does not exist locally or on any remote")`. **`worktree.create { branch }` (`checkout_worktree_on_branch`) calls this one**, because a user who pressed a listed row meant that branch, and a fresh empty branch wearing its name is never the right answer.
- `create_cutting_branch` — the same `Local` and `Remote` arms, and an `Absent` arm that requires `is_usable_branch_name` and cuts the name **exactly as given** from the base. **`branch.dispatch` (`cut_branch_for_dispatch`) calls this one**, which is what `branch_dispatch_cuts_a_named_branch_exactly_as_it_was_given` pins.

Both delegate to one private `checkout_branch(repo, branch, base_branch, teardown)` for the shared tail: unique directory name, `add_checkout_on_ref`, and the teardown marker. `Orchestrator` exposes one method per verb (`create_worktree_on_existing_branch`, `create_worktree_cutting_named_branch`) and adds nothing else.

`materialise_remote_branch(branch, remote, tracking_ref)` is handed the two strings it uses, not an enum to re-match: `bounded_git_fetch`, then the local ref at what came back, then `set_upstream`.

**`directory_name_for` is safe for the wider set.** It strips a leading `build/`, splits on `/` and joins with `-`; the `-n` collision suffix comes after. Git's ref rules mean the joined string is one non-empty path component that is never `.` or `..` and contains no separator. That suffix loop has one home, `unique_checkout_name(repo, stem, also_taken)`: a name git has registered a worktree under or that exists on disk is claimed, and `create` passes its own branch-namespace clause in as `also_taken`.

**Accepted knowingly: `worktree.create { branch }` does disk work under the app mutex.** Three steps, all on-lock:

1. A **forced** external-worktree rescan (`project_checkouts(project_id, true)`), because a verb about to act must decide against the checkouts that exist now, not a cached summary.
2. `ProjectCheckouts::holders`, which reads the primary checkout's HEAD with git2. Its doc comment says so: `holders` is the off-lock half **for a branch listing**, and the on-lock caller for this one-shot user action.
3. For a remote-only branch, `bounded_git_fetch` with a 30 s deadline.

This codebase otherwise pushes slow git off-lock, so all three are deliberate exceptions, taken because the work is bounded, user-initiated and one-shot rather than a poll, and because the same lock-held fetch precedent already exists in `restore_run_worktree`. If it becomes a problem the fix is the two-half pattern `worktree_finish` uses — resolve the scope under the lock, run `holders()` and the checkout in the deferred closure — not an unbounded or silent fetch. Nothing else may read "lock free" as an invariant of `project_checkouts` or `holders`.

### Teardown: one fact, one file, one reader

`branch_was_cut` was a return value nothing persisted, so the fact is written down where it survives, beside the checkout it describes:

```rust
pub enum BranchTeardown { DeletesBranch, KeepsBranch }
pub fn record_branch_teardown(worktree_path: &Path, teardown: BranchTeardown) -> Result<(), WorktreeError>
pub fn branch_teardown(worktree_path: &Path) -> Result<BranchTeardown, WorktreeError>
```

The marker is a file in the checkout's own git admin directory, reached through the `gitdir:` pointer in `<worktree>/.git` (a relative pointer, git 2.48+ with `worktree.useRelativePaths`, is resolved against the directory holding it). Git prunes that directory with the worktree, so the fact cannot outlive what it describes.

- `create` writes `DeletesBranch`; `create_on_existing_branch` writes `KeepsBranch`; `create_cutting_branch` writes `DeletesBranch` only for the `Absent` arm it cut.
- **The stamp is unwound when it cannot be written.** A registered checkout with no marker reads as `DeletesBranch`, so a marker write that fails over a borrowed branch would arm the next teardown to delete somebody else's work. `stamp_teardown_or_unwind` prunes the just-added registration and removes the directory, best-effort, and returns the write's own error.
- **`branch_teardown` has an error channel, and only one reading means `DeletesBranch`:** the admin directory was read successfully and the marker file is not in it — a checkout Build did not create, whose branch the human's chosen action speaks for. Every failure to read is an error, never a fallback.
- `run_finish_git_steps` reads the marker **before** `remove_registered_worktree` prunes the admin directory, and skips the branch deletion on `KeepsBranch`. `Merge` still merges; only the deletion is withheld.
- **`remove` reads the marker from the registration itself**, so no caller sequences the read. It reads it in the main repository's `worktrees/<name>` directory — which is still there when the working directory is already gone — and it reads it **before** `unregister`: a checkout that cannot answer is left standing whole (directory, registration and branch) rather than destroyed under a question nothing can be asked again afterwards. A branch that is already gone asks nothing of the marker.
- **`remove_keeping_branch` is the abandon-only verb that overrides it**: an abandoned run's work outlives the run so it can be re-attempted, which is a promise about the branch the checkout itself cannot make.
- `Orchestrator::discard_checkout(&worktree)` is one verb over `remove`. Its failure stays best-effort and quiet, and its log line names the branch left standing so the leak is diagnosable.
- `NamedBranchCheckout` carries `teardown` in place of `branch_was_cut`, and `BranchDispatchCreations.minted_branch` is deleted.

### The marker survives `restore`, or the caller vouches, or `restore` refuses

```rust
pub enum UnregisteredRestore { Write(BranchTeardown), Refuse }
pub fn restore(&self, worktree: &Worktree, when_unregistered: UnregisteredRestore) -> Result<Worktree, WorktreeError>
```

1. `worktree.path` exists → the `verify_existing_worktree` path; neither admin directory nor marker is touched.
2. Directory missing, registration still found → the admin directory is in the main repository, so the marker is still there. `restore` reads it before `prune`, re-adds the worktree, and records the same value into the fresh admin directory.
3. Registration gone → the environment cannot answer, and `when_unregistered` speaks: `Write(teardown)` re-adds and records that value; `Refuse` errors without fetching, re-adding, or writing a marker of either value.

**What the three callers can prove.** All three reach `restore` through `Orchestrator::restore_run_worktree`, and `app.rs` derives the argument in one place, `fn unregistered_restore_for(active: &ActiveRun) -> UnregisteredRestore`: a non-adopted run's checkout was made by `WorktreeManager::create` and nothing else, so `Write(DeletesBranch)` is the value the pruned registration carried; an adopted run's checkout may be one Build only checked out over somebody's branch, so it is `Refuse`.

## Components the finish path owns

- `WorktreeFinishAction::git_steps() -> FinishGitSteps` — the one place a finish action decides anything from its own kind, replacing the arms that each re-tested the action. `WorktreeFinishAction::verb()` beside it gives the word a failure names the finish by.
- `FinishContext { project_path, base_branch, record, worktree_path }` — built once in `run_finish_git_steps` and taken by every step, so no step's signature carries parameters it ignores.
- `finish_by_landing_then_removing(context, land: Option<FinishLanding>)` — merge passes `Some(merge_external_branch)`, delete passes `None`. The two actions differ by their *steps*, not by a boolean tag the function then re-reads.

## (d) SPA: pure model vs DOM

`spa/src/core/branchPickerModel.js`:

```js
export function branchPickerModel({ branches = [], query = "" })
// -> { rows: [{ key, name, intent, verb, detail, ahead, behind, stat, remote }],
//      cutNew: { preview } | null }
```

The model maps `intent` to a verb and nothing else. `fuzzyRank` and `branchNamePreview` are reused unchanged; `cutNew` is present only when the query is non-empty and matches no branch name exactly.

The two rows call the same RPC with different keys, which is what keeps the preview honest: a listed row sends `branch`, the cut-new row sends `name`.
