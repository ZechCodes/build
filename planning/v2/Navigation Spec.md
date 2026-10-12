# Project and workspace navigation

Planning amendment for **#523**, 2026-10-12. **Proposed; Zech decides the open
points below. No implementation is included.** Code inspected at `d4bd92fd`.
Companion: #522, `Project Workspace Spec.md` (separate planning branch).

## Decisions for Zech

1. **Separate the project link, sibling selector, and global Switch control.**
   Recommend all three: `Build / navigation-plan ▾     Switch`. The project
   name goes up immediately; the workspace name opens siblings; Switch shows
   projects and workspace destinations together. A caret attached to the project
   name alone saves space but keeps “go up” and “switch” ambiguous.
2. **Use last visits for shortcuts, not unread-driven reordering.** Recommend
   a small **Last visited** section in Switch, alphabetical project groups
   underneath, and unread badges that update without moving rows. Keep the
   rail's existing anchor order and quiet-work **Recent** disclosures unchanged.
3. **Make Up a return; make destination selection predictable.** Recommend
   the project breadcrumb restores that project's saved face (Tasks initially),
   while choosing a project in Switch/rail opens Tasks, and choosing a named
   workspace opens its existing first useful Changes/Files face. Browser Back
   restores the exact previous URL. An alternative is remembered faces for every
   destination, which needs broader new workspace-return state and is deferred.
4. **Phone height versus direct switching.** Recommend an **88px two-row bar**
   with the project, workspace/caret, lock and labeled Switch always present.
   It costs **44px more than a 44px single row**, or **50px more than today's
   38px toolbar**, reducing the chat/console overlay's usable height by the same
   amount. Alternative: a **44px single row**, keeping project/workspace/caret
   and lock, with **Switch projects…** in the local picker footer. That saves
   44px but makes cross-project moves three activations on phone; desktop keeps
   direct Switch. Prefer two rows for discoverability and consistent two-step
   navigation. Zech can choose the density tradeoff before N2 starts.

Vocabulary follows #522's recommendation: a **workspace** is the rail-based
view; the **project workspace** stands on original project sources; an
**isolated workspace** is a named checkout cut from those sources. Everyday UI
keeps the project name, the human workspace name, **New workspace**, and
**Workspaces**. Adopted checkouts keep their existing identity and annotation;
calling the view a workspace does not turn adoption into isolation.

## What exists, and what this amendment changes

The screenshot on #523 shows one button containing `Build / add-project-progress`.
The slash currently suggests hierarchy without providing a project target.

| Inspected source | Current behavior | Planned change or invariant |
| --- | --- | --- |
| `spa/src/core/toolbarRender.js`, `toolbarModel.js` | Workspace identity is one button; branch/legacy task name is static; tracker task shows only project selector | Distinct project link, workspace selector, explicit task ancestry |
| `spa/src/core/toolbar.js` | Workspace → Switch project → choose project → choose workspace: four activations; project choice only re-scopes the menu | Direct destination rows in global Switch: two activations, with search/scroll when needed |
| `toolbar.js`, `toolbarModel.js` | Cached workspace roster, status-first order, device-filtered project list; project return restores saved face | Keep cache authority/return behavior; define complete global list and stable ordering |
| `spa/src/core/router.js`, `projectModel.js` | Device-qualified project/workspace/branch/task routes; workspace task IDs already stay on workspace routes | Reuse identities, route factories and legacy resolution; no URL migration |
| `spa/src/core/projectRailState.js` | Explicit return remembers Tasks/Files/Workspaces; ordinary project links open Tasks | Preserve distinction; add Changes only with #522's Changes route |
| `spa/src/core/inbox.js`, `inboxProjects.js` | Alphabetical project blocks; watched activity rows in anchor order; Recent means dismissed or quiet for a day | Preserve these policies; this is not an exhaustive destination directory |
| `ARCHITECTURE.md`, “Render from cache” and “Surfaces” | Cache renders immediately; watched unread counts; rail and picker share names/keys | Required invariants for every child task |

The June UI brief describes Board → Task → Plan/Diff and promises everything
within two taps of the board. The running SPA has Inbox, project/workspace
rails, Tasks, Changes, Files, agent rails and a console. That old tree and its
unqualified tap claim are historical intent, not a description of this UI.
This amendment defines measurable navigation paths below. #522 owns the page
shape; in current code the project has Tasks/Files/Workspaces, no Changes or
console. Older comments calling original sources “not a place to work” are
also superseded only as #522's behavior ships.

## Places and routes

| Place | Meaning and address | Parent / lateral navigation |
| --- | --- | --- |
| Inbox / Projects rail | Account-wide activity, optionally device-filtered | Project heads and visible workspace rows open their destinations |
| Device | Machine holding the sources; settings/filter context | Keep existing device picker/settings; not a required stop between projects |
| Project workspace | The project's own workspace over its original sources; `project` route | Workspaces face manages named children; Switch reaches other projects |
| Named workspace | Existing `workspace_id`, under a project and device; `workspace` route | Project breadcrumb goes up; workspace selector lists siblings |
| Branch checkout | `branch` route, Changes/Files; a checkout view, not a new workspace entity | Project link goes up; Switch reaches workspaces |
| Task detail | `trackerTask`, or `workspace` with `tab: tasks, taskId` | Tasks crumb opens the host's Tasks list; host comes only from the URL |
| Legacy plan/task | Existing `task`/resolve route | Project link and current item link; keep resolver behavior |

Every key includes the device: existing `projectKey` is `(deviceId, projectId)`;
existing `workspaceKey` is `(deviceId, workspaceId)`, with `projectKey` retained
as parent metadata. Reuse `deviceKey.js` helpers, not a new triple-key encoding.
Never look up a row by displayed name,
bare `proj-1`, branch name, or conversation owner. Keep existing wire IDs and
`route.name` values. In #522 only the project Changes path is new.

### Tap budget, phone first

An activation means a tap/click on a control or destination. Search typing and
scrolling are additional effort; a two-activation route is not a promise that
an arbitrary destination is visible without either. Counts start on a work
page with the picker closed unless the row explicitly says otherwise.

| Move | Phone and desktop path | Activations | Keyboard |
| --- | --- | --- | --- |
| Workspace/branch/task → its project | Project name | 1 | Tab to project link, Enter |
| Workspace → sibling | Workspace name ▾, destination row | 2 | Focus selector, Space/Enter, arrows or search, Enter |
| Project workspace → named workspace | Workspaces ▾ shortcut, row; or Workspaces rail face, row | 2 | Focus shortcut/rail face, Enter, focus row, Enter |
| Project A → project B | Switch, B's Project workspace row | 2 | Ctrl/Cmd+K, type project, arrows, Enter |
| Workspace A → workspace on B | Switch, B / workspace row | 2 | Ctrl/Cmd+K, type project and/or workspace, Enter |
| Same move with Projects rail already open and target visible | Destination row | 1 | Tab to row, Enter |
| Same move via closed rail | Open rail, row if Projects is already its face | 2 minimum | Existing rail trigger, Tab, Enter |
| Task → its host's task collection | Tasks crumb | 1 | Focus Tasks link, Enter |
| Return after switching | Browser Back | 1 browser action | Browser's normal Back/Forward keys |

If the rail is on Inbox, a project is folded, or its row is inside Recent,
each disclosure/face change adds an activation. Switch is the consistent
direct route that does not depend on rail folds. No fake two-tap guarantee
through the existing nested menu or folded rail.

## Breadcrumb and toolbar

`/` is a separator, never part of a shared button. Link and selector hit areas
are visibly independent, with separate focus rings. Wrap the hierarchy in a
labelled navigation landmark and ordered list; controls adjacent to it are not
additional ancestors. Every displayed segment has a real link or button.
Current-page links retain the explicit route and `aria-current="page"`;
selecting them does not reset a file, task, agent, or scroll position.

| Standing page | Breadcrumb | Adjacent navigation |
| --- | --- | --- |
| Project Tasks, Changes, Files or Workspaces | `Build` (current workspace link) | `Workspaces ▾` sibling shortcut, `Switch` |
| Named workspace, any main face | `Build / navigation-plan ▾` | `Switch` |
| Project-hosted task | `Build / Tasks / #523` | `Workspaces ▾`, `Switch` |
| Workspace-hosted task | `Build / navigation-plan ▾ / Tasks / #523` | `Switch` |
| Branch checkout | `Build / branch-name` | `Workspaces ▾`, `Switch` |
| Legacy plan/task route | `Build / task-title` | `Workspaces ▾`, `Switch` |
| Inbox/account/device settings | Keep the surface's own title; no invented active project breadcrumb | Global `Switch` remains available |

The current workspace button announces “Switch workspace in Build; current:
navigation-plan.” The project link announces “Open project workspace Build.”
The task leaf links to its current URL and uses its number; its heading carries
the full title. Until task metadata arrives, use a task-ID fallback, not a made-up
number. A branch leaf is a self-link to the current branch URL (slashes remain
part of its name, not additional breadcrumb segments).

Tasks is an actual parent link: on a workspace task it removes `taskId` while
retaining that workspace and its Tasks layout; on `trackerTask` it goes to the
project's Tasks face. An assignee change cannot change this ancestry. Source
directories and file paths stay in their existing face-level controls, not in
this bar. Settings remains a modal over its host, retaining the host breadcrumb.

### Width, long names, and the rail

Design at **390 × 844** first, stress at **320 × 740**, then **1280 × 800**.
All navigation targets have at least 44 × 44 CSS-pixel touch areas. Reserve
space for Switch and the existing workspace lock or branch verb; never merge
the project and workspace hit areas to save width.

- Recommended rule: toolbar container below **560px** uses two **44px** rows,
  with no vertical gap, making **88px**. Row 1 is project / workspace ▾; row 2
  places the lock and **Switch** at the right. Task ancestry sits at row 2's
  left. A project workspace uses row 1 for its name and row 2 for **Workspaces ▾**
  and Switch. Above 560px, use one 44px row unless zoom/text metrics require
  wrapping. Do not hide the project link in `…`.
- Exact phone allocation (zero horizontal safe-area inset, collapsed Inbox):
  current shell gutters take 20px, the floating Inbox control needs 48px at the
  toolbar's left, and right padding takes 10px. At **390px**, usable row width
  is **312px**: project **80px**, gap **4px**, slash **12px**, gap **4px**, workspace
  button **212px**. At **320px**, usable width is **242px**: project **64px** and
  workspace button **158px**, with the same separator/gaps. Workspace width
  includes 16px horizontal padding, a 12px caret and 4px inner gap, leaving
  **180px / 126px** for the name. Project text has 16px total padding, leaving
  **64px / 48px**. For intermediate widths, project is 80px when usable width
  is at least 280px, otherwise 64px; workspace takes the remainder. Neither
  target shrinks below 44px. Keep existing safe-area/Inbox clearance in the
  calculation; if text zoom or insets prevent fitting, wrap rather than overlap.
- Row 2 reserves **44px lock + 4px gap + 80px Switch**. On a workspace task,
  the left **Tasks / #523** group uses 44px Tasks, a 12px slash and two 4px gaps;
  its leaf is 60px at 390 and 44px at 320, separated from actions by 4px.
  IDs/titles ellipsize within the leaf. Branch verbs
  take the analogous action space and may require another row at large text
  sizes. The lock keeps its existing accessible label/state; Switch stays a
  visible word, not an unexplained icon. The global Inbox trigger remains at
  its existing left position, outside these three navigation targets.
- If Zech chooses the single-row alternative, reuse row 1's allocation after
  reserving **48px** for lock plus gap. Workspace name space becomes **132px /
  78px** at 390/320. Switch moves to the local footer, labeled **Switch projects…**;
  the keyboard shortcut still opens global destinations. Project pages use a
  separate 44px sibling-chevron target after their project link. Task ancestry
  still requires a second row when present; no parent link silently disappears.
  Update phone tap-budget assertions from two to three for cross-project moves.
- Overlay offsets must use the actual toolbar height. Today phone chat and full
  console use `--toolbar-h:38px`; changing only the bar would put content under
  it. N2 must update these geometry consumers together, including zoom/wrapping
  and the global Inbox trigger alignment. The 88px choice is an explicit cost
  to chat space, not a cosmetic change with no impact below it.
- Ellipsize names independently; project may shrink before workspace, but keeps
  a visible label and a separate target. At 320px, wrapping controls beats
  reducing targets or forcing horizontal scrolling. Selected names and full
  context appear in the open picker; accessible names always retain full text.
  Native hover titles may help desktop but are not the only full-name access.
- Normal project names need no device crumb. For duplicate project names show
  a small device qualifier beside the name, wrapping as needed; expose full
  device identity in destination rows. The device picker/settings remains the
  real way to change device scope, without an extra hierarchy level here.
- On desktop, an open Projects rail keeps its folds, scroll and active marker.
  The toolbar remains available in the content pane; responsiveness follows its
  available width, including when the agent rail is open, not just viewport width.
- On phone, only one navigation overlay is interactive. Opening Switch from
  an inbox overlay closes that overlay without navigating or losing its state.
  Selecting a rail/picker destination dismisses the phone overlay. Desktop's
  docked rail stays open. Escape dismisses only the top overlay and restores
  focus to its trigger; it does not navigate to Inbox.

## Two entry points to one destination picker

**Workspace name / Workspaces ▾** opens the picker scoped to the current
project. It shows a **Project workspace** return row, the current named workspace
marked **Current**, and every other live named workspace, including unwatched
ones. It offers **All projects** to widen the scope in place and **Manage
workspaces** to open #522's Workspaces face. The project return row is not a
synthetic workspace entity and is not duplicated in the management list.

**Switch** opens the same picker directly at all projects, across devices,
independent of the Inbox device filter. The heading says **All projects · All
devices**; a device filter is an optional local picker control, not a silent
inheritance. Clearing it restores the full catalog. Current route identity
never changes when a filter changes. Project rows navigate; they never secretly
mean “expand to choose a second destination.”

Illustrative open global picker (square brackets below are controls):

```text
Switch to                              [Close]
[Search projects or workspaces               ]
All projects · All devices

Last visited
  Docs / release-notes                 2 unread
  Build / navigation-plan             Current

Build                                      4
  [Project workspace]                        1
  [navigation-plan]                    Current
  [toolbar-fix]                              3
Docs                                       2
  [Project workspace]
  [release-notes]                             2
```

The project heading supplies context; the **Project workspace** row is the
explicit destination to the project's own page. Workspace rows navigate
directly, so opening Docs is never a prerequisite for opening release-notes.
No hover submenu, project drill-down, or horizontal swipe is required.

### Search, ordering and counts

- Global empty query: up to **five Last visited destinations**, newest visit
  first, then projects alphabetically using existing locale/device/key tie
  breaks. Within a project, named workspaces are alphabetical with stable ID
  tie breaks. The Project workspace row comes first. A shortcut can also occur
  in its project group; both refer to the same destination key/current state.
- Local picker: Project workspace, current child (when any), then alphabetical
  siblings. Status is a secondary label, not a reason to reorder ready/failed
  rows under a finger. Finished/deleted workspaces are excluded; provisioning
  and failed ones remain navigable to their existing status view.
- Search matches project, human workspace name, and device label. A project
  match exposes its Project workspace and matching-context children; a child
  match retains the project heading. Use existing fuzzy matching, relevance
  then alphabetical/key tie breaks. Search results omit Last visited duplicates.
  Empty search says “No matching projects or workspaces”; an empty local roster
  says “No workspaces in this project” when no query is present.
- Last visited is local navigation history, stored separately from replicas in
  `localUiState`, scoped to the account/device-qualified destination. Record
  an actual change of project/workspace destination, including breadcrumb/rail
  navigation and browser Back/Forward. Project-hosted tasks count as visiting
  that project; workspace-hosted tasks count as visiting that workspace. Branch,
  legacy plan and account/settings pages do not add destination shortcuts. A
  first direct entry counts once; a reload of the same destination does not.
  Track the previous destination per browser tab so repeated visits across
  tabs do not suppress one another; order the shared history by visit time with
  key tie breaks. Menu opening, hover, search, filters, polls, renames and face
  changes never count. Store at most 20 destination keys; display five that still exist
  in the known roster. Reset with account reset. No unread/read cursor changes.
  The rail's **Recent** remains quiet activity, not this visit history.
- Badges use the same watched unread accounting as the rail, including task
  ownership without double-counting a task row and its owning workspace.
  Project headings show the aggregate once; Project workspace rows show only
  that destination's own contribution, not the full project aggregate again.
  This already includes the pinned project agent and watched tasks with no
  workspace owner (`inboxProjectAgent.js` / existing task-unread attribution).
  #522's project-owned agents join that contribution when supported. Do not
  reuse the stale toolbar comment that a project has no conversation of its own.
- Unread counts help choose where to go, not position. They update in place.
  Freeze ordering while the picker is open unless the user changes query/scope;
  reconcile names/status/counts by stable key. New rows can appear at the end
  until reopening; removal keeps focus on the nearest valid row. Never reset
  query, text selection, list scroll or chosen project because a feed tick lands.

### Phone and keyboard operation

On phone, the picker is a full-width sheet with visible title, Close, search
and scrollable rows; respect safe areas and virtual-keyboard viewport height.
On desktop, use an anchored dialog/popover wide enough for project context and
the same list. It is a navigation dialog with ordinary destination links and a
search field, not a menu of mutation commands. Do not nest buttons inside links.

Tab reaches every control; Space/Enter opens picker buttons; arrows move the
active result; Enter opens it; Escape dismisses and returns focus. Ctrl/Cmd+K
opens global Switch from non-editable app chrome. Do not intercept it inside
inputs, contenteditable, code editors, terminal, composition, or another modal;
their key handling wins. The visible control works everywhere those shortcuts
cannot. Modal phone focus is contained; desktop dialog also has a coherent
tab cycle, with outside click dismissal and focus return. Opening by touch
does not summon the keyboard until search is tapped; keyboard opening focuses
search. Navigation focuses the destination heading/main landmark rather than
leaving focus on a removed result. Native modified-click/open-in-new-tab works
on links using canonical hashes. Do not hijack browser Back/Forward keys.

The ordinary workspace toolbar DOM and tab order is **project link → workspace
button → lock → Switch**; Shift+Tab reverses it, with no positive `tabindex`.
Project page order is project → Workspaces → Switch. Task links follow their
host identity before lock/Switch; branch action follows its identity before
Switch. From focused project: Enter goes up; Tab then Space opens siblings;
from workspace: Tab skips through the lock to Switch, then Space opens all
projects. In either picker, typing filters, Down selects the first result,
Up/Down move results without editing the query, Enter navigates, Escape returns
to the exact trigger. Tab also provides an ordinary path through search,
filter controls, destination links and Close; no arrow-key-only controls.

### Relationship to the rail and #522

The picker and Projects face are **different tools over shared cached records**:
picker = complete destination directory; rail = watched activity/triage with
folds, counts and Recent. Share identity, human names, route factories and
unread calculations, not a single pre-filtered row list or ordering policy.
Quiet or unwatched workspaces must remain findable in Switch even when absent
from the rail. #522's Workspaces face is the management inventory of named
children, with filters/status/size/reclaim; it has no Project workspace self row.
It and the sibling list share the same cached roster and open-route factory.
Their selected filters and management controls need not match the picker.

New workspace and existing Open review / View pull request entry points must
remain reachable during replacement of the current toolbar menu. Retain them
in a separated **Workspace actions** footer of the local picker initially,
with the existing capability/standing-workspace checks; global search results
never carry mutation actions. This preserves today's review affordance without
adding deletion, reclaim, or other duplicate management UI. A later relocation
requires a separately reviewed replacement, not silent removal in this work.

## Navigation state and failure cases

- Open a new project destination using `projectRoute`; open a named workspace
  using `workspaceRoute`. First-use project landing is Tasks; first useful Git
  directory lands on Changes, other directories on Files, as today.
- Choosing a **Current** destination is a no-op beyond closing the picker;
  its link uses the exact current URL, not the default destination factory.
  Do not reset its file, task, source, agent, scroll or visit timestamp. The
  same rule applies to its Last visited shortcut and matching rail row.
- The **ancestor project link** and local **Project workspace** return row use
  `projectReturnRoute`; missing/invalid saved state falls back to Tasks. On the
  project workspace itself its identity is a self-link to the current route.
  Extend return validation for project Changes only after #522 implements it.
- Explicit task/comment/file/line/source/commit/agent URLs are authoritative.
  Do not carry a source ID, file path, task, commit, or agent from A into B.
  Browser Back/Forward restores the original URL and existing per-surface UI
  state; switching must preserve existing drafts and console/session state.
  A direct jump adds one history entry; searching/changing picker scope adds none.
- Navigation closes the picker; poll/push does not re-scope it. Keep in-progress
  query/focus through cache and UI-store refreshes; an older async return cannot
  overwrite a newer navigation. Treat Last visited separately from ephemeral
  open/focus state so another tab's visit cannot steal this tab's focus.
- Render known names/rosters/counts immediately from cache. Do not hide or
  disable destinations based on connecting/offline state, fetch on picker open,
  or wait for RPC to navigate. If roster knowledge is absent, say “No cached
  workspaces yet”; absence of a cached list is not proof of an empty project.
  Ordinary cache sync supplies later rows; a failed read is not a deletion.
- A rename updates labels without changing keys/history. A confirmed removal
  removes the destination/visit shortcut. If it disappears during activation,
  retain the current page and explain “Workspace is no longer available,” with
  a project link. A removed current workspace/deep link retains project ancestry
  and the existing unavailable surface rather than bouncing to another device.
  Missing project names fall back to the route's project ID, never the last
  scoped project's label. Unknown device labels fall back to a distinguishing ID.

## Sequenced child tasks

These are proposed child boundaries, **not dispatched work**. Each gets one
branch and one review after Zech decides. Estimates include tests and review
fixes; one half-day is about four focused hours. Total: **9 half-days**.

| Child | Size | Depends on / parallelism | Observable acceptance and tests |
| --- | --- | --- | --- |
| N1 — Shared destination catalog and route contract | 2 half-days | First; can run alongside #522 bridge/console work | Cached projects and live named workspaces have existing device-qualified keys, shared display/route/unread helpers; unwatched destinations are searchable; duplicate IDs/names cannot cross devices. Carry cached roster presence/observation into the catalog: current feed defaults to `[]` and alone cannot distinguish unknown from confirmed empty. Read canonical list cache metadata, never connection state or a new RPC. Test unknown → empty and failed refresh retaining rows in `navigationModel.test.js`; extend `toolbarModel.test.js`, `projectRailState.test.js`, `router.test.js`. No speculative project Changes route. |
| N2 — Real breadcrumbs and local sibling navigation | 2 half-days | N1; one owner for toolbar controller/render/CSS | Separate project link works in one activation on workspace/branch/task; local sibling choice in two; host Tasks ancestry and phone wrapping correct; lock/review/branch verb retained. Extend `toolbarDom.test.js`, `toolbarReviewEntry.test.js`; add `navigationBreadcrumbLayout.test.js` with B1–B4 below. |
| N3 — Direct global Switch, search and Last visited | 2 half-days | N2 (same toolbar ownership); no dependency on new #522 wire verbs | Global list selects B/workspace without entering B, supports all devices, local visit state, stable unread and focus under pushes; keyboard and dismissal complete. Add `navigationPicker.test.js`, `navigationPickerLayout.test.js` with P1–P5. |
| N4 — Rail, management face and project Changes integration | 1 half-day | N3 and #522 project Changes/return-state, Workspaces-face and final vocabulary children; counts for new project-owned agents wait for their #522 feed integration | Same destination opens from picker, Projects rail and management face; no original-source fake child; current markers/phone overlay behavior correct; project Up restores Changes after #522 enables it. Extend `projectsFaceLayout.test.js`, `workspaceNavWiring.test.js`; add `navigationIntegrationLayout.test.js` with I1–I3. |
| N5 — End-to-end navigation acceptance and docs reconciliation | 2 half-days | N4; fixtures/test scaffolding may be prepared alongside N2/N3 without editing their production files | Run the full matrix with real renderers and seeded cache, preserve legacy deep links/drafts/history, fix integration failures; update ARCHITECTURE's current-state descriptions and obsolete toolbar/router comments only for shipped behavior. Add `navigationHistoryLayout.test.js` with H1–H3 and run all SPA gates. |

N1/N2/N3 can ship useful navigation before #522's full project workspace.
N4 waits for the relevant #522 children, not unrelated source/agent implementation.
N4 should reuse #522's finished management face and count model, not implement
them again. Agree actual child IDs/dependency edges when creating the tasks.

### Named browser acceptance matrix

Use the existing Vitest/Chromium `spa/test/browser/layoutHarness.mjs` against
isolated seeded cache and production modules, never a paired client or the
user's browser. Every scenario runs at **phone 390 × 844** (touch) and
**desktop 1280 × 800**, in both themes where layout/color matters. B4 and P4
also run at **320 × 740** and desktop with both side rails open.

| ID / test name | Required observable result |
| --- | --- |
| B1 `project crumb goes up without opening picker` | From A/child, one project tap returns to saved project face; no dialog, no other device visited |
| B2 `workspace selector opens sibling in two activations` | Open picker then choose sibling; URL and Current mark name the correct workspace; no project-page intermediate |
| B3 `task breadcrumbs preserve route host` | Project task and workspace task have correct Tasks-parent links; assignee/push changes do not change host; branch with slashes remains one leaf |
| B4 `long ancestry keeps separate reachable targets` | 80-character names, duplicate project/device labels, task suffix, lock, branch verb, and open rails do not overlap, clip controls, or cause horizontal page scrolling; 44px targets and full accessible names |
| P1 `switch jumps across projects without a project visit` | A/child → Switch → B/child is two activations and one history entry; unseen-by-rail/unwatched B child remains findable |
| P2 `search distinguishes same names and devices` | Two devices with `proj-1`, same project/workspace names and encoded Unicode names resolve to intended device; match includes project/device context |
| P3 `cache tick preserves query focus and destination` | While typing and selecting a row, push unread/name/status/new row updates; query/caret/scope/scroll/order stay stable; no extra list RPC; cached away-device destinations remain usable |
| P4 `picker works by touch and keyboard` | Touch opens sheet without keyboard; search/Close/results stay visible in reduced visual viewport; Tab/arrows/Enter/Escape, modified click and focus return work; shortcut does not steal editor/terminal keys |
| P5 `last visited differs from quiet Recent` | Real navigation updates five shortcuts, reload retains them, poll/face changes do not; account reset clears them; watched unread updates cannot reorder rows or mark content read |
| I1 `all entry points share destinations` | Picker, Projects rail, Workspaces face open same named workspace route and labels; project destination uses project route with no fake workspace ID |
| I2 `rail folds and overlays survive switching` | Desktop rail retains folds/scroll, phone overlay closes on choice, only top overlay consumes Escape; active project/workspace markers follow route |
| I3 `project Changes returns after elevation` | After #522: visit project Changes, enter child, tap project crumb; correct project/source Changes returns, including non-Git source fallback |
| H1 `back restores exact prior place and drafts` | A Files path/line/source/agent → B → Back restores A's URL and existing draft, then Forward returns B; task/comment/commit deep links remain valid |
| H2 `rename removal and empty cache stay honest` | Renames preserve identity, confirmed deletion drops shortcut without selecting adjacent row accidentally, absent cache differs from empty result; project fallback remains available |
| H3 `existing review and legacy routes remain reachable` | Open review/View pull request and lock/branch verb still work under their current capabilities; old branch/task/plan links resolve as before; current leaf click does not reset context |

Implementers follow repository test-first rules, plus `npm run lint`, `npm test`
(includes Chromium layout tests), and `npm run build`, all under `nice -n 10`.
Run gitleaks before pushing, semgrep on changed tracked JS/CSS before ready,
and `git diff --check`. This planning change is validated as documentation;
the browser scenarios above are specified, not claimed as executed.
