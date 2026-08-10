# Stage 01 — The rail's selection algorithm: never drop live, unread, or unreviewed work

## Goal

Make `spa/src/core/rail.js` — the single module that decides WHICH entries the
side rail shows and in what order — able to serve both the existing per-project
rail and the flat "everything" rail that Stage 02 builds. Three changes, all
pure and unit-tested, no view changes:

1. **A never-drop floor.** Anywhere an agent is running, anywhere something is
   waiting on you that you have not seen, and anywhere there are changes you
   have not reviewed, is always an entry — regardless of age and regardless of
   the backfill minimum.
2. **The project's main checkout becomes a rail entry.** A new `mainEntry()`
   turns a `primary_changes` record (plus the run that owns that checkout, when
   one has adopted it) into an entry of the same shape as run/plan/worktree
   entries, so Stage 02 can put main branches in the flat list.
3. **`railEntries()` can select across every project at once**, and can be
   handed main-checkout entries as candidates.

After this stage the app still looks exactly as it does today except for the
floor in (1), which is a deliberate, visible improvement to the existing
per-project rail: a worktree with an agent painting in it, or with uncommitted
work you have never looked at, now gets a row instead of hiding in the
`Worktrees ›` fold.

## Context a cold agent needs

**Which app.** The web client is `spa/` — a no-framework, vanilla-ESM, Vite
bundled SPA. Tests are vitest (`cd spa && npx vitest run`). This stage touches
exactly two files: `spa/src/core/rail.js` and `spa/test/rail.test.js`.

**What the rail is.** `#sidebar-rail` in `spa/index.html`, rendered by
`spa/src/views/sidebar.js` (DOM wiring) from HTML built by
`spa/src/core/sidebar.js` (pure builders) over a model whose entry *selection*
comes from `spa/src/core/rail.js` (pure). Data arrives from the shared
`board.list` + `project.list` poller in `spa/src/core/taskFeed.js`, whose
snapshot is `{plans, runs, externalWorktrees, primaryChanges, projects}`.

**What `rail.js` does today** (read the file's header comment before editing —
it explains the intent and must stay accurate):

- `runEntry(run)` / `planEntry(plan)` / `worktreeEntry(worktree)` normalise a
  feed record into an entry: `{kind, id, project_id, name, branch, state,
  working, terminal, needsYou, changedAt, attention, status, route, …}`.
- `dotState(entry)` → `"working"` (an agent is mid-flight), else `"unseen"`
  when `attention.seen` is falsy, else `"seen"`.
- `railEntries({runs, plans, worktrees, projectId, nowMs, minimum})` filters
  every list to `projectId`, keeps worktrees only when
  `worktree.attention?.interacted`, then selects:
  1. everything `working` or whose `changedAt` is within `DAY_MS`;
  2. if fewer than `RAIL_MINIMUM` (5) chosen, tops up with `needsYou` entries,
     newest first;
  3. if still fewer, tops up with the most recently resumed;
  and finally sorts ascending by `attention.resume_at` (oldest stretch of work
  at the top, so a resumed entry lands at the bottom and nothing above it
  moves), breaking ties by name.
- `railWorktrees({worktrees, entries, projectId, by})` returns the worktrees
  the entries did *not* show — the contents of the `Worktrees ›` fold.

**Feed field shapes** (from `bridge/src/app.rs`, `board_list` /
`external_worktrees_json` / `primary_changes_json`):

- run: `{run_id, project_id, goal, branch, state, needs_attention, primary,
  can_finish, state_changed_at, updated_at, attention, stat:{ahead, behind,
  comparison_ref, insertions, deletions, uncommitted:{files_changed,
  insertions, deletions}}}`.
- plan: `{plan_id, project_id, goal, state, needs_attention, can_archive,
  archived_at, state_changed_at, updated_at, attention}`.
- external worktree: `{worktree_id, project_id, name, branch, agent_working,
  can_finish, dirty_files, ahead, behind, comparison_ref, upstream,
  base_branch, uncommitted:{files_changed, insertions, deletions}, attention}`.
- `attention` is always `{resume_at, interacted, seen}`.
- primary change (one per project): `{project_id, branch, path?, upstream,
  comparison_ref, ahead, behind, files_changed, insertions, deletions, run_id}`
  — `run_id` is the run that has adopted the primary checkout, or `null`.
  **It carries no `attention` and no `agent_working` of its own**: an agent
  running in a project's primary checkout adopts it as a run first (see
  `bridge/src/app.rs`, `primary_run_of`), so the owning run is where liveness
  and read-state live.

**Who calls `railEntries` today.** Only `buildSidebarModel` in
`spa/src/core/sidebar.js`, once per project, after removing the primary run
from the run list when a `primary_changes` record exists to carry its dot on
the checkout line. That call site keeps working unchanged: the new parameters
are optional and default to today's behaviour.

## What to build

All edits in `spa/src/core/rail.js`. Written test-first — for each numbered
item below, add its tests to `spa/test/rail.test.js`, watch them fail, then
implement.

### 1. `hasUnreviewedChanges(entry)` and `mustShow(entry)` (both exported)

```
hasUnreviewedChanges(entry):
  the entry has a diff nobody has signed off on —
  dotState(entry) !== "seen"  AND
  ((entry.status?.insertions ?? 0) + (entry.status?.deletions ?? 0) > 0
   || (entry.status?.ahead ?? 0) > 0)

mustShow(entry):
  entry.working                                   // an agent is running here
  || (entry.needsYou && dotState(entry) !== "seen")  // unread, waiting on you
  || hasUnreviewedChanges(entry)                  // changes you have not reviewed
```

Both are pure functions of one entry. Give each a short comment saying what
question it answers, in the voice of the rest of the file.

### 2. The floor inside `railEntries`

- Worktree candidates: today they are filtered by `worktree.attention?.interacted`
  *before* normalisation. Change to: normalise every worktree, then keep the
  ones where `entry.attention?.interacted || mustShow(entry)`. An agent running
  in a worktree you have never touched in Build, or uncommitted work sitting in
  one, is exactly what the rail exists to surface.
- Selection pass 1 becomes `if (entry.working || finishedToday || mustShow(entry)) take(entry)`.
- Passes 2 and 3 (the backfill to `minimum`) are unchanged, and the final sort
  is unchanged.
- Update the module header comment and the comment above pass 1 so they still
  describe what the code does: the floor is "alive, or finished within the day,
  or it needs you / holds unreviewed work", and the backfill only tops up a
  thin list.

### 3. `mainEntry({ project, primaryChange, run })` (exported)

Turns a project's primary checkout into an entry. `project` is a
`project.list` record (`{project_id, name}`), `primaryChange` is that project's
`primary_changes` record, `run` is the raw run that owns it (the run whose
`run_id === primaryChange.run_id`, or the run with `primary: true` for that
project) or `null`.

```js
{
  kind: "main",
  id: project.project_id,          // the row's identity IS the project
  project_id: project.project_id,
  project_name: project.name || "",
  name: primaryChange.branch || "",   // main rows are named by their branch
  branch: primaryChange.branch || "",
  path: primaryChange.path || null,
  state: "",
  working: run ? <run is mid-flight> : false,
  terminal: false,
  needsYou: !!run?.needs_attention,
  changedAt: <run's state_changed_at ?? updated_at, else null>,
  attention: run?.attention || {},
  run_id: run?.run_id || primaryChange.run_id || null,
  status: {
    ahead: primaryChange.ahead ?? null,
    behind: primaryChange.behind ?? null,
    comparisonRef: primaryChange.comparison_ref || primaryChange.upstream || null,
    insertions: primaryChange.insertions ?? 0,
    deletions: primaryChange.deletions ?? 0,
    changesLabel: "uncommitted",
  },
  route: { name: "project", projectId: project.project_id, tab: "changes" },
}
```

Reuse the existing private helpers rather than duplicating them: `ms()` for the
date parsing, and the same `WORKING_RUN_STATES` set `runEntry` uses for
`working`. The status shape is deliberately identical to `checkoutStatusHtml`'s
input in `spa/src/core/sidebar.js` so the same renderer can draw it.

Note the consequence, and make sure the tests pin it: a main checkout with a
dirty working tree has `attention.seen` falsy (no attention record at all when
no run owns it), so `mustShow` is true and it can never be dropped — which is
the "this includes the main branch of project" half of the goal.

### 4. `railEntries` selects across every project

- `projectId` becomes optional: when it is `null`/`undefined`, no project
  filtering happens and candidates come from every project. Keep the existing
  behaviour byte-for-byte when a `projectId` is passed.
- Add a `mains = []` parameter: already-built `mainEntry` objects, added to the
  candidate list alongside runs, plans and worktrees (filtered by `projectId`
  the same way when one is given). They go through the identical selection and
  sort; nothing pins them to the top.
- `railWorktrees({worktrees, entries, projectId, by})` gets the same optional
  `projectId` treatment, so Stage 02 can ask for one combined leftover list.
- The `chosen` map is keyed `${kind}:${id}`, which stays unique across projects
  because run/plan/worktree ids are globally unique and a main row's id is its
  project id under a distinct `main:` prefix.

## Tests to write first (`spa/test/rail.test.js`)

The file already has `run()`, `plan()`, `worktree()` factories and a fixed
`NOW` / `ago(hours)` helper — extend them, do not restate them. Add a
`primaryChange()` factory and a `project()` factory in the same style.

New cases, each named as a sentence about behaviour (match the file's voice):

**The floor**
- shows a worktree with an agent in it even though Build never touched it
  (`agent_working: true`, `attention: {interacted: false, seen: false}`, old
  `resume_at`) — it is an entry, not a leftover.
- shows a worktree holding uncommitted work nobody has looked at, however old.
- keeps a stale, clean, already-seen worktree out of the entries (proving the
  floor did not simply admit everything) — it still lands in `railWorktrees`.
- shows a week-old run that is waiting on you and unseen, when five newer
  entries would otherwise have crowded it out (build 6+ candidates so the
  backfill cannot be what saves it).
- does not force-show a seen entry that merely has a diff.
- leaves the existing ordering assertions passing (do not weaken them).

**`hasUnreviewedChanges` / `mustShow`**
- unit cases over hand-built entry objects: working → shown; unread + needsYou
  → shown; unseen with `+/-` → shown; unseen with `ahead > 0` → shown; seen
  with a big diff → not shown; clean and seen → not shown.

**`mainEntry`**
- carries the branch as its name, the project's name, and the checkout's
  ahead/behind/± as its status, and routes to the project's Changes tab.
- with no owning run: `working` false, `needsYou` false, `attention` empty, and
  `mustShow` true as soon as the checkout is dirty.
- with an owning run mid-flight: `working` true, `changedAt` from the run, and
  `dotState` is `"working"`.
- a clean main checkout with no run is not force-shown.

**Cross-project selection**
- with no `projectId`, entries come from every project, ordered by resume point
  across all of them (not grouped).
- with a `projectId`, the result is identical to today's (regression).
- `mains` participate in the same selection: a dirty main is always chosen; a
  clean, quiet main is subject to the ordinary alive/today/backfill rules.
- `railWorktrees` with no `projectId` returns leftovers from every project,
  ordered by name.

## Definition of done

- `cd spa && npx vitest run` passes in full (every existing test too — the
  sidebar tests in `spa/test/sidebar.test.js` exercise `railEntries` through
  `buildSidebarModel`, and one of them, "hands worktrees the entries did not
  show to the Worktrees row", uses a `worktree()` fixture with
  `dirty_files: 0` and zero diff, so the floor must not disturb it; if any
  sidebar test does break, fix the *fixture's* intent, not the floor).
- `spa/src/core/sidebar.js` and `spa/src/views/sidebar.js` are untouched.
- The module header comment in `rail.js` still describes what the code does.
- Run `semgrep` and `gitleaks` before committing (project rule), then commit.

## Assumptions

Recorded because the reviewer was asked but planning did not wait:

- **"Unreviewed changes" has no persisted marker anywhere in the app.**
  `spa/src/core/reviewMemory.js` is per-session, in-view state only. So it is
  defined here as *"there is a diff and the entity is not `seen`"* —
  uncommitted `+/-`, or commits ahead of the comparison ref. If this proves
  noisy in a repo full of stale branches, the tuning knob is one clause: drop
  `ahead > 0` from `hasUnreviewedChanges` and keep only uncommitted work.
- **The floor applies to both rail modes**, because the goal asks the flat mode
  to use "the same algorithm as the individual projects" and there is one such
  algorithm. The visible consequence in per-project mode is stated above.
- **A main checkout has no liveness of its own.** An agent running against a
  project's primary checkout adopts it as a run first, so `mainEntry` reads
  liveness and read-state from the owning run and reports `working: false` when
  there is none.
