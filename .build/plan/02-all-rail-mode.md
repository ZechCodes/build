# Stage 02 — The "All" rail mode: every worktree, issue and main branch in one list

## Goal

Give the side rail a second mode. Today it is one block per project; the new
mode drops the grouping and shows one flat list of everything Build knows
about — runs, issues (plans), worktrees, and every project's main checkout —
selected and ordered by the same algorithm the per-project rail uses. A small
switch in the rail header moves between the two, and the choice is remembered.

The selection algorithm itself, including the guarantee that anything with an
agent running, anything unread and waiting on you, and anything holding
unreviewed changes can never be dropped, landed in Stage 01. This stage is the
model, the markup, the switch, and the styling.

## Context a cold agent needs

**Which app.** `spa/` — a no-framework, vanilla-ESM, Vite-bundled SPA. Tests
are vitest: `cd spa && npx vitest run`. A single test file:
`npx vitest run test/allRail.test.js`.

**The rail today.**

- `spa/index.html` holds the shell: `<aside id="sidebar">` with `.side-top`
  (logo + collapse), `.side-nav` (Notifications, Account), `<div
  id="sidebar-rail">` (everything this stage renders), and `.side-status`.
- `spa/src/views/sidebar.js` is the wiring: it subscribes to the shared feed
  (`spa/src/core/taskFeed.js` → `{plans, runs, externalWorktrees,
  primaryChanges, projects}`), builds the model, writes `innerHTML` into
  `#sidebar-rail` (preserving `scrollTop`), rebinds click handlers on every
  paint, and calls `wireRailDoneControls`. State it owns: `closedProjects`
  (persisted, `build.sidebar.closedProjects`), `wtOpen` (per session),
  `pendingDone` + `doneErrors` (optimistic Done).
- `spa/src/core/sidebar.js` is the pure HTML builder:
  `buildSidebarModel(...)` → one model per project `{project_id, name, unread,
  entries, worktrees, primary}`; `projectHtml(m, ui)` → one project block;
  `sidebarHtml(model, ui)` → the header (`Projects` label + `#side-add`) plus
  every block. Module-private helpers worth reusing: `dotHtml(entry)`,
  `entryStatusHtml(status)`, `statusTitle(status)`, `entryRow(entry, ui)`,
  `checkoutStatusHtml(primary)` (exported).
- `spa/src/core/rail.js` (as left by Stage 01) exports `railEntries`,
  `railWorktrees`, `runEntry`, `mainEntry`, `dotState`, `mustShow`,
  `hasUnreviewedChanges`, `RAIL_MINIMUM`. `railEntries` now accepts
  `mains` and treats a missing `projectId` as "every project"; so does
  `railWorktrees`.

**Row contracts already wired in `views/sidebar.js`** — reuse them verbatim so
the flat rows need no new routing:

| markup | click goes to |
| --- | --- |
| `.srow[data-run]` (+ `data-tab`, `data-project`) | `{name:"task", projectId, id, tab}` |
| `.srow[data-plan]` (+ `data-project`) | `{name:"plan", projectId, id, tab:"conversation"}` |
| `.srow[data-wt]` (+ `data-project`) | `{name:"worktree", projectId, worktreeId}` |
| `.srow[data-main]` | `{name:"project", projectId, tab:"changes"}` |
| `.srow[data-wtline]` | toggles that key in `wtOpen` and repaints |
| `[data-done-run] / [data-done-plan] / [data-done-worktree]` | `wireRailDoneControls` in `spa/src/core/railDone.js` |

**Active-row state** comes from `ui`: `{closed, wtOpen, activeRunId,
activePlanId, activeProjectId, activeMainProjectId, activeWorktreeId}`, all
computed in `views/sidebar.js` from `App.route`. The flat mode uses the same
fields; `activeMainProjectId` is what lights a main row.

**CSS** lives in `spa/src/styles.css`; the rail's rules are the block around
`#sidebar` / `.srow` / `.sdot` / `.sproj*` / `.swt-*` (roughly lines 849–960,
plus a mobile override near line 959 and a `.smain-line` rule near 1113).

## What to build

Test-first throughout. Four new/changed source files.

### 1. `spa/src/core/railMode.js` (new, pure)

The remembered mode, in the shape of `spa/src/core/readState.js` (pure
functions over an injected Web-Storage-like object):

```js
export const RAIL_MODE_KEY = "build.sidebar.mode";
export const RAIL_MODES = ["projects", "all"];
export function loadRailMode(storage)   // → "projects" | "all"; anything unrecognised, missing, or a throwing storage → "projects"
export function persistRailMode(mode, storage)
```

### 2. `spa/src/core/allRail.js` (new, pure)

```js
export function buildAllRailModel({
  projects, runs, plans, externalWorktrees, primaryChanges,
  readIds, nowMs, pendingDone = new Set(),
})  // → { entries, worktrees, unread }
```

- Apply the same `pendingDone` suppression `buildSidebarModel` does: an entry
  is invisible while `pendingDone` holds `run:<id>` / `plan:<id>` /
  `worktree:<id>`. (Main rows have no Done control, so they are never
  suppressed.)
- Build one `mainEntry` per project that has a `primary_changes` record,
  passing the run that owns it — the run whose `run_id === primaryChange.run_id`,
  falling back to the project's run with `primary: true`.
- Drop that owning run from the run list, exactly as `buildSidebarModel` does
  for the checkout line: a primary run is its main row, never a second row.
  Unlike the per-project model, do this whenever a main row exists to carry it.
- `entries` = `railEntries({runs, plans, worktrees: externalWorktrees, mains,
  nowMs})` with **no** `projectId` — one selection and one backfill across
  everything, not five per project.
- `worktrees` = `railWorktrees({worktrees: externalWorktrees, entries})` with no
  `projectId`: one combined leftover list, ordered by name.
- `unread` = runs + plans that are `needs_attention` and not in `readIds`,
  across every project (the same question `buildSidebarModel`'s badge answers).
- Stamp each entry with the project name it belongs to (look it up from
  `projects` by `project_id`; `mainEntry` already carries `project_name`) so the
  renderer can label a row that no longer sits under a project heading.

### 3. `spa/src/core/sidebar.js` — the flat renderer and the mode switch

Add to the existing module (so the private `dotHtml` / `entryStatusHtml` /
`entryRow` helpers are reused rather than copied):

- **`railHeadHtml(ui)`** (private): the header both modes share — the mode
  switch on the left, `#side-add` on the right. The switch is two buttons,
  `<button class="railmode" data-railmode="projects">Projects</button>` and
  `<button class="railmode" data-railmode="all">All</button>`, with `active` on
  the current one and `aria-pressed` set accordingly. `sidebarHtml` uses it in
  place of today's `Projects` label, keeping `#side-add` exactly as it is.
- **`entryRow(entry, ui, opts)`**: extend, do not fork.
  - New `kind: "main"` arm: active when `entry.id === ui.activeMainProjectId`,
    data attribute `data-main="<project_id>"`, branch name rendered mono (like
    `.smain-line` does today), status via `entryStatusHtml`, no Done control.
  - New `opts.showProject`: when true, the row carries
    `<span class="sproj-tag">project name</span>` before its title, so a flat
    row still says where it lives. Escape it like every other string.
  - Everything else — the dot, the status, the Done buttons, the `active`
    class, the `data-project` attribute — is unchanged and shared.
- **`allRailHtml(model, ui)`** (exported): `railHeadHtml(ui)` + a
  `<div class="side-all">` holding every entry row (`showProject: true`) plus
  the combined `Worktrees ›` fold. The fold reuses the existing `.swt-line` /
  `.swt-list` / `.swt-item` markup with the key `__all`
  (`data-wtline="__all"`), each item labelled with its project the same way,
  and each keeping its `data-project` so routing works. Empty model → the same
  `Nothing running.` empty state the project blocks use.

### 4. `spa/src/views/sidebar.js` — the switch

- `let railMode = loadRailMode(localStorage);`
- In `draw()`: when `railMode === "all"`, build with `buildAllRailModel` and
  paint `allRailHtml`; otherwise keep today's `buildSidebarModel` +
  `sidebarHtml` path. `scrollTop` preservation, `setBadge`, `#side-add`, the
  `data-run`/`data-plan`/`data-wt`/`data-main`/`data-wtline` handlers,
  `wireRailDoneControls`, and `paintDoneErrors` are all mode-independent — they
  run exactly as they do now, after the paint.
- Wire `[data-railmode]`: set `railMode`, `persistRailMode(railMode,
  localStorage)`, `draw()`. Do not navigate; switching modes must not move the
  user off the surface they are on.
- The `[data-chev]` / `[data-open]` project-header handlers only exist in
  projects mode; guard their `querySelectorAll` loops the way the others
  already tolerate an empty match (they do — `forEach` over an empty NodeList
  is a no-op), and make sure `$("#side-add").onclick` still binds in both modes
  (it does, because `railHeadHtml` renders it in both).

### 5. `spa/src/styles.css`

Beside the existing rail rules:

- `.railmode` buttons: small, quiet, in the `.side-head` row, the active one
  carrying the accent treatment already used by `.srow.active`
  (`var(--accent-soft)` / `var(--accent)`). Keep the header's height so the
  rail does not jump when switching.
- `.sproj-tag`: dim, small, mono, ellipsised, never squeezing the row title out
  (`.srow .stitle` is already `flex:1; min-width:0`).
- `.side-all`: the flat list container — the same left padding rhythm the
  `.sproj-body` rows have, minus the project block's border/indent.
- A main row in the flat list reads like `.smain-line` does today (mono branch,
  slightly smaller) without the `margin-left:16px` indent, which only made
  sense under a project name.
- Check the narrow-viewport override near line 959 still holds.

## Tests to write first

**`spa/test/railMode.test.js`** (new)
- defaults to `projects` when nothing is stored, when the stored value is
  unrecognised, and when the storage throws.
- round-trips `all` through `persistRailMode` / `loadRailMode`.

**`spa/test/allRail.test.js`** (new) — model, then HTML. Follow the fixture
style of `spa/test/sidebar.test.js`: a fixed `NOW`, an `iso(hoursAgo)` helper,
and `run()` / `plan()` / `worktree()` / `primaryChange()` factories over two
projects (`p1 relaydb`, `p2 dotfiles`).

Model:
- puts work from every project in one list, ordered by resume point across
  projects — assert an interleaving that grouping could not produce.
- gives every project with a `primary_changes` record a main row when that
  checkout is dirty, and lights it with the owning run's dot when a run has
  adopted it.
- shows a main checkout with an agent running in it, and one with uncommitted
  work, even when there is plenty of newer work above them.
- does not list the primary run twice — its row is the main row.
- counts unread work across all projects for the badge.
- omits entries whose confirmed Done work is still running (`pendingDone`).
- hands every leftover worktree, from every project, to one combined list
  ordered by name.

HTML (`allRailHtml`):
- renders the mode switch with `all` marked active, and still renders
  `#side-add`.
- each row carries its routing attribute (`data-run` / `data-plan` /
  `data-wt` / `data-main`) and its `data-project`.
- a flat row shows the project it belongs to.
- a main row shows its branch, its `↑/↓/+/−` status, and no Done control.
- the combined `Worktrees ›` fold is folded by default and lists its items
  under `wtOpen` containing `__all`.
- escapes every string it renders (mirror the existing escaping test — a
  project named `<script>evil</script>` and a goal with an `<img onerror>`).
- the empty model says `Nothing running.`

**`spa/test/sidebar.test.js`** (existing): add one case that `sidebarHtml`
renders the mode switch with `projects` active. Everything else there must keep
passing untouched.

## Definition of done

- `cd spa && npx vitest run` passes in full.
- Switching modes keeps you on the same surface, keeps the active row
  highlighted, and survives a reload.
- Verify in a real browser with the `spa:verify` skill: both modes paint, a
  project's main branch appears in All mode when its checkout is dirty, an
  agent-running row pulses, clicking each kind of row lands on the right
  surface, and the mode survives a reload.
- Run `semgrep` and `gitleaks` before committing (project rule), then commit.

## Assumptions

Recorded because the reviewer was asked but planning did not wait:

- **The switch lives in the rail header**, where the `Projects` label is today,
  as a two-way `Projects | All` control, remembered in `localStorage` under
  `build.sidebar.mode`, defaulting to `projects`.
- **A flat row keeps its project as a dim label** on the row, since nothing
  groups it anymore.
- **One combined `Worktrees ›` fold** in All mode, across every project,
  ordered by name — the fold is a place to look something up, not a feed.
- **Main rows sort into the list** by the same resume-point key as everything
  else rather than being pinned above it.
- **The backfill minimum (`RAIL_MINIMUM`, 5) applies once globally** in All
  mode, not per project — the floor from Stage 01 is what guarantees coverage,
  and the backfill only keeps a quiet list from looking empty.
- **A main row has no Done control.** A project's primary checkout is not
  something you finish.
