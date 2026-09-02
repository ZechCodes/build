# Stage 04 — The isolation setting: account default, per-project override, locked when unavailable

## Goal

Make the choice real on the bridge: an account-level `isolation` setting, a
per-project override, one resolver that every creation site uses, refusal of `cow`
wherever the probe says no, a silent-but-recorded fallback if the environment
changes later, and the wire fields the SPA needs. Spec:
`planning/v2/Work Isolation Spec.md` §5 and §6.

## Context a cold agent needs

- **Settings today** (`bridge/src/app.rs`): `settings_get` (≈6579) returns
  `projects_dir`, `default_harness`, `claude_mode`, `codex_mode`; `settings_set`
  (≈6593) parses every field before applying any and refuses an empty set;
  `persist` (≈3592) writes the config JSON with `projects_dir`, `default_harness`,
  `router_model`, `projects: [{path, base_branch}]`; the loader near ≈2223 reads
  them back (unknown `default_harness` logs and keeps the default — copy that
  idiom). `AppState` holds `default_harness: AgentProvider` (≈1872) — put
  `isolation: Isolation` beside it.
- **Projects** are `struct Project` (≈999): `id`, `name`, `repo_path`,
  `base_branch`, `orch`, caches. `add_project` (≈3610) builds the orchestrator;
  `project_json` (≈6400) is the wire row; `project_set_remote` (≈6440) is the one
  existing per-project mutation and the pattern for the new one; `"project.set_remote"`
  is dispatched near ≈5861. `project.list` is polled by the SPA feed.
- **Creation sites** pass `Isolation::Worktree` literally since stage 1: search
  `Isolation::Worktree` in `app.rs` — the bare-worktree create, the named-branch
  create, `dispatch_run`, boot-recovery restore (≈2535), and the two other
  `restore_run_worktree` calls (≈5118, ≈10044).
- **Thread events**: `crate::thread::ThreadEventKind` (e.g. `WorktreeRecreated`
  used at ≈2540 with a message) — reuse an existing kind for the fallback note;
  do not add a kind unless none fits.
- `WorktreeManager::availability()` exists (stage 3) and is cheap enough to call
  on every `settings.get`/`project.list`.

## What to build

### 1. State and persistence (§5.1, §5.2)

- `AppState.isolation: Isolation` (default `Worktree`), persisted as top-level
  `"isolation"`; loaded like `default_harness`.
- `Project.isolation: Option<Isolation>`, persisted per project entry as
  `"isolation"` only when `Some`; loaded with the project.
- `fn resolved_isolation(&self, project_id: &str) -> (Isolation, Option<String>)`
  returning the isolation to create with and, when it downgraded a `Cow` choice,
  the probe's reason. This is the one resolver; no other code reads
  `self.isolation` or `project.isolation` to make a decision.

### 2. Creation sites (§5.3)

Replace every `Isolation::Worktree` literal in `app.rs` with the resolver's answer.
When the resolver returns a reason, after the create succeeds append the thread
event `"Created a git worktree: copy-on-write isolation is unavailable here — <reason>"`
to the entity's conversation (the run's, or the worktree agent's thread for a bare
worktree) and `eprintln!` the same line.

### 3. Wire (§5.4)

- `settings.get`: add `"isolation"` and `"isolation_available"` (first registered
  project's `availability()`; none → `{"cow": false, "reason": "no project registered yet"}`).
- `settings.set`: accept `"isolation"` through one private `accept_isolation`
  (wire-word parse, unknown refused in the `default_harness` refusal shape,
  `"cow"` refused when the given availability is `Err` with
  `"copy-on-write isolation is unavailable: <reason>; locked to worktrees"`) that
  `project.set_isolation` shares. Keep the "nothing to set" refusal accurate (add
  the field to its condition). With no project registered, `settings.get` answers
  `isolation_available` as `{"cow": false, "reason": "no project registered yet"}`;
  that sentence lives in `app.rs`, not in the probe.
- `project.list` rows: `"isolation"` (own, `null` when inheriting),
  `"isolation_default"` (the account setting), `"isolation_effective"`,
  `"isolation_available"` (this project's probe).
- `"project.set_isolation"` → `project_set_isolation(params)`: `project_id`
  required; `isolation` is `"worktree"`, `"cow"` or `null`; `cow` refused when this
  project's probe is `Err` with the same sentence; persist; return the row.

Serialize `IsolationAvailability` to the wire as `{"cow": bool, "reason": string|null}`
in one helper used by both `settings_get` and `project_json`.

### 4. Nothing else changes

No store schema change, no run/plan/archive record change (§5.5). Grep to confirm.

## Tests (write first)

App tests use `state.handle(req("settings.get", json!({})))` as the existing
settings tests (≈19145–19450) do.

- `settings.get` reports `"isolation": "worktree"` and an `isolation_available`
  object on a fresh state; after `settings.set {isolation: "cow"}` on a machine
  whose probe passes, `get` reports `"cow"`; on a machine whose probe fails the set
  is refused with the locked sentence and `get` still says `"worktree"`. Make the
  probe outcome deterministic in tests by pointing the worktrees root at a temp dir
  on the same volume as the temp project (the passing case) and — for the failing
  case — by registering a project whose `.git` is a file (a linked worktree
  registered as a project), which fails check 1 on every platform.
- `settings.set {isolation: "telepathy"}` is refused; `settings.set {}` still
  refuses "nothing to set".
- Persistence round-trip: set account `cow` and a project override `worktree`,
  reload from the config file, both survive; an unknown persisted value loads as
  the default and logs.
- `project.set_isolation` with `null` clears the override; `project.list` shows
  `isolation`, `isolation_default`, `isolation_effective` and
  `isolation_available` correctly for inherit / override / locked.
- Resolver: account `cow` + project `worktree` → `Worktree`; account `worktree` +
  project `cow` → `Cow`; `cow` chosen but probe fails → `Worktree` with a reason.
- Fallback event: create a bare worktree with `cow` chosen and the probe failing;
  the thread carries the "Created a git worktree: copy-on-write isolation is
  unavailable here" line and the checkout is a linked worktree.
- End to end on APFS (skip with reason otherwise): `settings.set cow`, dispatch a
  run, `Isolation::of(run.worktree.path) == Cow`, the run's review diff and
  `board.list` stat work, `run.finish merge` merges the clone's branch into the
  base and removes the directory.

## Done when

Tests green on APFS and ext4; clippy/fmt clean; spec §8.1 holds (`Isolation::Cow`
outside the module appears only in the resolver, the two refusals and tests);
`settings.get`, `project.list` and `project.set_isolation` behave as §5.4.
