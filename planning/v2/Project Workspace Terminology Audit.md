# Project workspace terminology audit — #537

Audited against the vocabulary and baseline code-correction inventory in
[Project workspace](Project%20Workspace%20Spec.md), after #529–#536, #541–#543
and #618 merged at `dd7c1b0644ec8adc33d960d22098b9924c884fdc`.
This audit covers display copy, accessible names, help and source documentation.
Validation evidence is pending the task's focused suites, browser captures and gates.

## UI-string audit

| Surface | Copy and accessible-name contract | Source checked |
| --- | --- | --- |
| Project bar and inbox head | The project name; breadcrumb accessible name `Open project workspace <name>` | `spa/src/core/toolbarRender.js`, `inboxProjects.js` |
| Original-source navigation destination | `Project workspace` in sibling and global switchers, including Last visited; the project name remains the group context | `spa/src/core/toolbar.js`, `navigationPickerRender.js` |
| Global and sibling controls | `Switch project or workspace`; `Switch workspace in <project>` with the current child's name when present | `spa/src/core/toolbarRender.js` |
| Named workspace rows and bars | Given workspace name, shared `workspaceDisplayName` fallback; no added `isolated` prefix | `spa/src/core/workspaceModel.js`, `toolbarModel.js`, `navigationModel.js`, `projectPageModel.js` |
| Creation | `New workspace` actions and heading; help explains a separate checkout of the project sources | `spa/src/core/createWork.js`, `toolbar.js`, `inboxProjects.js`, `spa/src/views/projectView.js` |
| Project rail | `Tasks`, `Changes`, `Files`, `Workspaces`, `Project settings`; `Terminal` is a console toggle outside the face tablist | `spa/src/core/directoryRail.js`, `projectRail.js` |
| Named workspace rail | `Changes`, `Files`, `Tasks`, `Workspace settings` | `spa/src/core/directoryRail.js`, `workspaceRail.js` |
| Project agent rail | Pin says `Project agent`; workers use their given names; creation says `Add agent` | `spa/src/core/agentRailModel.js`, `agentRail.js` |
| Worker creation and empty worker roster | Help explains work in original project sources and shared files, Git index and current branch | `spa/src/core/agentRail.js` |
| Source Git actions | `Commit`, `Fetch`, `Pull`, `Push`, `Stash`, `Apply stash`, branch selector; `Sync base` separately identifies the configured base | `spa/src/core/gitPane.js`, `gitRender.js`, `spa/src/views/projectChanges.js` |
| Source context | `Original sources · shared checkout`; current branch and configured base stay distinct | `spa/src/views/projectChanges.js`, `spa/src/core/projectChangesModel.js` |
| Plain source | `This source is not a Git repository.`; Files and Terminal remain usable, with no initialization offer on project Changes | `spa/src/core/projectChangesModel.js`, `spa/src/views/projectChanges.js` |
| Older bridge | `Update this device to use project Changes.`, `Update this device to use project Terminal.`, `Update this device to use project agents.`; feature rendering remains based on cached support | `spa/src/core/projectChangesModel.js`, `projectChangesRpc.js`, `projectConsole.js`, `projectTerminalRpc.js`, `agentRail.js` |
| Inbox face | `Projects` remains the entity collection label | `spa/src/core/railMode.js`, `spa/src/core/inboxView.js`, existing inbox tests |
| Markdown references and legacy URLs | Project/entity labels and task/agent/workspace names remain tied to the referenced entity; project references open canonical Tasks, workspace references keep the named child host | `spa/src/core/markdownLinks.js`, `referenceTargets.js`, `router.js`, `routeResolve.js` |

Named workspace name fallbacks keep the established display-name behavior:
unnamed adopted checkouts may display their folder, and identity-only navigation
fixtures may display the addressed workspace ID. The default fallback remains
`Workspace`; this copy pass does not change naming or navigation behavior.
The ordinary Git `Pull` action stays distinct from `Sync base`. `Project agents`
remains a roster/overview collection label; `Project agent` names the pin.

## Baseline comment inventory

The spec's coordinates refer to the earlier baseline, so this inventory records
symbols and files rather than depending on those old line numbers.

| Baseline finding | Current correction |
| --- | --- |
| `router.js:97`: main pane described as Workspaces | `projectPage` documents Tasks default and Changes, Files and named-child Workspaces |
| `router.js:99`: source checkout described as no place to work | `projectPage` describes original sources as a shared place to work |
| `router.js:196`: checkout described as having no surface | `projectSurface` describes original-source Changes and Files while retaining legacy landings |
| `router.js:289`: both Tasks faces described as having two layouts | `placeInSurface` documents project Dashboard, workspace List and shared Board availability |
| `toolbarModel.js:38`: project described as having no conversation | The legacy menu projection documents counters from feed rows, including project-owned conversations |
| `toolbarRender.js:1/:27`: one trigger described as a breadcrumb | Renderer and `toolbarBreadcrumbs.js` document and render linked URL ancestry separately from selectors |
| `toolbarModel.js:1`: single-picker header | Header describes linked ancestry, local selector and global switcher with device-qualified destinations |
| `toolbarRender.js:21`: legacy task confused with trackerTask | Breadcrumb model distinguishes legacy `task` leaves from `trackerTask` and workspace task ancestry; `toolbarModel.js` explicitly qualifies its legacy task feed row |

Additional stale source comments corrected: `workspaceModel.js` now distinguishes
an older synthetic source-checkout row from a named child without denying that
original sources are usable; `projectPageModel.js` names the Workspaces face,
`projectModel.js` documents Changes and the source console, and `agentRail.js`
distinguishes the initial coordinator's device preference from worker defaults.
`toolbar.js` calls #618's switcher glyph a left chevron.

The current #522 amendment blocks in the UI Design Brief, Scope, Changes View
and Agent Surfaces specs now describe the settled, implemented #529–#536 scope.
Their historical design and lifecycle prose remains historical. `ARCHITECTURE.md`
describes the actual toolbar modules, navigation vocabulary and project workspace
surfaces without changing headings or anchors.

## Contracts retained deliberately

- RPC namespaces, method names, capabilities and parameter keys remain unchanged,
  including `project.*`, `workspace.*`, `tasks.*`, `project_role`, `worker`,
  `coordinator`, `project_id`, `workspace_id`, `source_id` and `source_path`.
- Route kinds remain `project`, `workspace`, `branch`, `trackerTask`, legacy `task`
  and `resolve`. Device-qualified URLs, legacy aliases and source/file/line/commit/
  agent queries keep their existing spelling and compatibility behavior.
- Persisted owner and workspace IDs, cache record names/addresses, UI draft and
  local-return storage keys remain unchanged. No synthetic workspace is added.
- `@project`, `@workspace`, `@agent` and existing file/commit reference syntax remain
  unchanged. Branch pages retain Changes/Files checkout semantics.
- Bridge prompts, worker authority, source-lock behavior, source lifecycle and
  workspace-cut behavior remain the previously reviewed implementation.

## Validation evidence

Pending root execution and recording: red copy/accessibility assertions followed
by green focused router, rail, navigation, reference/legacy URL and worker-help
suites; phone/desktop light/dark captures; lint and build; semgrep, gitleaks and
`git diff --check`. The full suite is reserved for the project's serial gate.
