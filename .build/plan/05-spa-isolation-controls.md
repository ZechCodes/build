# Stage 05 — The isolation controls: account settings panel and project settings select

## Goal

Let the reader choose the isolation from the web client: a **Work isolation**
panel on the Settings page (account default) and a **Work isolation** select in
the project settings sheet (per-project override), both disabled with the
bridge's reason when copy-on-write is unavailable. Spec:
`planning/v2/Work Isolation Spec.md` §7, on the wire of stage 4 (§5.4).

## Context a cold agent needs

- **Which app.** `spa/` — a no-framework, vanilla-ESM, Vite-bundled SPA. Tests are
  vitest: `cd spa && npx vitest run`. The bridge is reached with
  `App.call(method, params)`; views pass `callRpc` in for testability.
- **The pattern to copy** is `spa/src/core/defaultHarness.js` + its test
  `spa/test/defaultHarness.test.js`: a pure `…Of(settings)` reader, a
  `…PanelHtml()` that renders empty, and a `mount…(host, {callRpc})` that paints
  from `settings.get`, saves with `settings.set` on change, repaints from the
  bridge's answer, and shows the bridge's refusal text on failure. The Settings
  page (`spa/src/views/settings.js`) includes the panel with
  `${defaultHarnessPanelHtml()}` and calls `mountDefaultHarness($("#root"), …)`
  after `refresh()`.
- **Project settings sheet:** `spa/src/sheets/projectSettings.js` (64 lines) paints
  read-only name/path/base-branch fields, an editable origin remote, and a Save
  remote button, from a `project.list` row. Its test is
  `spa/test/projectSettings.test.js`. The row now carries `isolation`,
  `isolation_effective`, `isolation_available` (stage 4).
- **Copy rules** (user preference, repo-wide): plain words, no metaphors, one term
  per concept — "isolation", "git worktree", "copy-on-write clone".
- The `spa:verify` skill (if present in this checkout) drives a browser pass; use
  it at the end. Run `semgrep` and `gitleaks` before each commit.

## What to build

### 1. `spa/src/core/isolation.js` (pure)

```js
export const ISOLATIONS = [
  { id: "worktree", label: "Git worktree" },
  { id: "cow", label: "Copy-on-write clone" },
];
export function isolationOf(settings)                      // "worktree" | "cow"; anything else → "worktree"
export function isolationLockReason(available)             // available?.cow === false → available.reason || "unavailable on this device"; else ""
export function isolationOptionsHtml(selected, available, { inheritLabel } = {})
  // <option>s; "cow" gets `disabled` when locked; inheritLabel adds a leading "" option "Account default (<label of selected default>)"
export function isolationPanelHtml()                       // the Settings panel, select disabled "loading…"
export async function mountIsolation(host, { callRpc })    // paint from settings.get, save via settings.set {isolation}, repaint from the answer
```

Escape every string that came from the bridge (`esc` from `core/text.js`) — the
reason is bridge text.

Hint copy in the panel (verbatim): "A copy-on-write clone starts with the
project's build caches already in place and keeps its own git repository. A git
worktree shares the project's repository and starts empty." When locked, a second
line: "Locked to git worktrees on this device: <reason>."

### 2. Settings page

Insert `${isolationPanelHtml()}` directly after `${defaultHarnessPanelHtml()}` and
call `await mountIsolation($("#root"), { callRpc: (m, p) => App.call(m, p) })`
right after `mountDefaultHarness`.

### 3. Project settings sheet

Between the base-branch field and the origin remote, a field **Work isolation**
with a `<select id="psisolation">` built by
`isolationOptionsHtml(project.isolation || "", project.isolation_available,
{ inheritLabel: <label of project.isolation_effective when inheriting> })`. Saving
on `change`: `callRpc("project.set_isolation", { project_id, isolation: value || null })`,
then repaint the sheet from the returned row; a refusal shows in `#pserr` and the
select repaints from `project.list`. The lock reason renders under the select as a
`.dim` line when locked. The Save remote button keeps its own job.

### 4. Projects list rows (Settings page)

Append the effective isolation label after the base branch in each `.projrow`
(same `dim` style) so the account page shows what each project will do.

## Tests (write first)

`spa/test/isolation.test.js`:
- `isolationOf` for `{isolation: "cow"}`, `{}`, `{isolation: "nope"}`, `undefined`.
- `isolationLockReason` for `{cow: true}`, `{cow: false, reason: "r"}`, `{cow: false}`, `undefined`.
- `isolationOptionsHtml` marks the selected option, disables `cow` when locked,
  and leads with the inherit option when asked; a reason containing `<` is escaped.
- `mountIsolation` with a fake `callRpc` (like `defaultHarness.test.js`): paints
  from `settings.get`; on change calls `settings.set` with `{isolation}` and
  repaints from the answer; on refusal shows the message and repaints from
  `settings.get`; when locked the select still allows choosing `worktree`.

`spa/test/projectSettings.test.js` (extend): the sheet renders the select with the
inherit label built from `isolation_effective`; changing it calls
`project.set_isolation` with `null` for the inherit option and `"cow"` otherwise;
a locked project renders the disabled option and the reason line.

Settings page: the projects list shows the effective isolation label (extend the
existing settings-page test if one renders `#projlist`; otherwise add a small one).

## Done when

`npx vitest run` green; a browser pass shows the panel under Default agent, the
select in the project sheet, the disabled option plus reason on a locked project,
and a saved choice surviving a reload; `semgrep` and `gitleaks` clean; the SPA
security checklist unchanged at 100/100.
