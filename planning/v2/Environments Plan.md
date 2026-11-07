# Environments: a layer above projects

Planning for **#1028**, 2026-11-07. **Planning only; nothing here is
implemented.** Code inspected at `f1376d41` (`origin/dev`). Wire today is
`3.36.0` (`bridge/src/api/mod.rs:78`).

Companions: `Project Workspace Spec.md` (#522), `Navigation Spec.md` (#523,
#618, #631), `Project Workspace Terminology Audit.md` (#537), `workspaces.md`,
`Store Migration Spec.md`, `Project Sources Security Checklist.md`,
`Secrets Security Checklist.md`, `Tasks Spec.md` (scope rules, lines 1567–1582).

Zech's ask, in one paragraph: a layer above projects, **environments**, owns
source directories and secrets. Projects are created from environments and may
add source directories; workspaces and projects are otherwise unchanged. The
user creates **workspaces**, not projects, through a wizard that picks an
environment (a diverged project is listed as its own environment), with the
last day's recents on top and "new environment" as a second page; project
creation is implicit. A settings page manages environments and existing
projects. The top nav's switcher lists projects and offers "new workspace"; the
open workspace sits beside it and lists the project's workspaces by recency.
Projects and workspaces get badges in nav and inbox. Four stages: MVP, full
feature, polish, cleanup.

---

## 1. Model

### 1.1 Today

- A **project** (`bridge/src/app/projects/mod.rs:135`) is `{id: proj-N, name,
  repo_path, base_branch, is_git, sources: Vec<ProjectSource>, next_source,
  orch, isolation: Option<Isolation>}`. The first source is the project's home;
  `name` is the home folder's name and is read-only in settings
  (`spa/src/sheets/projectSettings.js:22-25`). Source ids `source-N` are never
  reissued (`next_source`, persisted at
  `bridge/src/app/config/persistence.rs:45`).
- A **workspace** (`bridge/src/workspace.rs:84`) is `{id, project_id, name,
  root, status, directories: Vec<WorkspaceDirectory>, isolation, managed,
  created_by_agent, locked}`, each directory cut from one `WorkspaceSource
  {id, name, mount, path, is_git, base_branch}` (`workspace.rs:110`). New
  workspaces take the project's sources at that moment
  (`AppState::sources_for`, `bridge/src/app/projects/mod.rs:566`); later
  source edits are forward-looking. The manifest `.build-workspace.json`
  (`workspace.rs:37`) records `project_id` and the `source_id` of each
  directory, so one source folder already backs any number of isolated
  checkouts (worktree or rift, `bridge/src/isolation/`).
- **Secrets** are scoped by `GrantScope::Agent {agent_id, conversation_id,
  generation} | Project {project_id}` (`bridge/src/secrets/model.rs:13`);
  project membership is revalidated per read and project scope is revoked on
  project deletion (Secrets checklist rows 8 and 11).
- **Tasks** are keyed by the project's canonical home path, not `proj-N`:
  `tracker_tasks.project_key` with `UNIQUE (project_key, number)`
  (`bridge/src/store/schema.rs:226-245`); `tracker_project_path` resolves it from
  `project.repo_path` (`bridge/src/app/tracker/mod.rs:719`). A task belongs to
  exactly one project and never moves (Tasks Spec 1567–1582).
- Several stores are keyed by the project's **path**, not its id, because an
  id is minted per boot: tasks (`project_key`), persisted runs and plans
  (`PersistedRun.project_path`, `PersistedPlan.project_path`,
  `bridge/src/store/entities.rs:40-47,99-112`), the project conversation
  owner's scratch folder (`scratch_dir`,
  `bridge/src/app/projects/conversation.rs:27-41`), and the path→id map
  `task_project_maps` (`bridge/src/app/tracker/read.rs:39-50`), which is
  one-to-one only because no two projects share a path today. Secrets,
  workspaces, compaction and entity bindings use the `proj-N` id.
- Every agent MCP tool takes its project from the caller's binding
  (`ProjectRegistry::project_id_of`, `bridge/src/app/projects/project_registry.rs:344`);
  no agent tool carries a project id (`bridge/src/mcp.rs:1521-1526`, `327-335`).
- Projects and ids are **per device**; the SPA keys everything by
  `deviceKey(deviceId, projectId)` (`spa/src/core/deviceKey.js:11`).

### 1.2 Proposed

```
device
 └─ environment  env-N          owns: name, sources[], next_source, secrets (env scope),
    │                                  default isolation, task numbering (see 1.6)
    ├─ project    proj-N        owns: environment_id, added_sources[], isolation override,
    │   │                              compaction policy, project agent + workers, orchestrator
    │   └─ workspace ws-…       owns: project_id, directories (one per effective source),
    │                                  isolation, name, lock          (unchanged)
    └─ project    proj-M  (diverged: added_sources non-empty)
```

**Environment** (new, `bridge/src/app/environments/`): `{id: env-N, name,
sources: Vec<ProjectSource>, next_source, isolation: Option<Isolation>}`. It is
the owner of source directories and of the one `source-N` counter, and the
scope of environment secrets. The environment's first source is its **home**
(the folder its name follows, and the task key, 1.6). Overlap refusal (a path
that is, contains, or sits inside another registered source; a remote already
registered; a path inside the secrets vault; `source_request`,
`bridge/src/app/projects/requests.rs:285-365`) moves up to the environment and
applies across every environment and every project's added sources on the
device.

**Project**: keeps `id`, `name`, `orch`, `isolation`, compaction, agents, and
gains `environment_id: String` and `added_sources: Vec<ProjectSource>`. Its
**effective sources** are `environment.sources ++ project.added_sources`, in
that order, and that is what `sources_for` returns. `repo_path`/`base_branch`
become derived from the environment home (kept as fields for one release for
the code that reads them, then removed in Stage 4). Project `added_sources`
ids are minted from the **environment's** counter so a workspace's
`directories[].source_id` is unique within the environment and a removed id is
never reissued anywhere in it.

**Base project.** Every environment has at most one project with no added
sources, its *base project*. "Create a workspace in environment E" means
"create it in E's base project, creating that project first if E has none".
That is the implicit project creation Zech asked for. The base project's name
is the environment's name.

**Diverged project.** A project **diverges from its environment iff
`added_sources` is non-empty**. Nothing else counts: an isolation override, a
compaction override, a project agent roster or project-scoped secrets do not
make a project its own thing in the wizard, because none of them changes what
a workspace cut from it contains. Detection is on the bridge:
`project.diverged = !project.added_sources.is_empty()`, carried on the wire as
`diverged: bool` on every project row, never computed client-side from
sources. A diverged project is listed in the wizard and the switcher as
"Environment › Project" and behaves exactly like an environment as a target.
Removing its last added source makes it the base project if the environment
has none, otherwise the bridge refuses the removal with "E already has a base
project; delete this project or keep one added source" (one base per
environment keeps "create in E" unambiguous).

**Workspaces are unchanged.** A workspace still names its `project_id` and
snapshots the project's effective sources when it is cut. `environment_id` is
derivable from the project and is added to `workspace.list` rows for the SPA's
grouping; the on-disk manifest gains the field lazily on next write and is not
required to load.

### 1.3 Secrets: inheritance and override

- New `GrantScope::Environment { environment_id }`. Readable by any current
  project agent, worker, or workspace agent whose project belongs to that
  environment; membership (`project.environment_id`) is revalidated on every
  read, like project membership today (row 8). Revoked when the environment is
  deleted (row 11 analogue). Agent and Project scopes are unchanged.
- **Override**: when a project-scoped and an environment-scoped secret share a
  `name`, attachment resolution for that project's agents picks the project
  one. Nothing is merged; the environment value is simply not offered for that
  name in that project. The chip shows "overrides environment" so it is
  legible. No override exists in the other direction.
- The device vault, sealing, audit and release path are untouched; only the
  scope enum, its validation (`model.rs:153`), the list/revoke filters and the
  `attached_secrets.md` wording change. See 7 for the checklist delta.

### 1.4 Isolation

`isolation` on the environment is the default for its projects; the project's
override stays (`configured_isolation`, `bridge/src/app/config/mod.rs`). The
settings page shows "Inherit environment setting" where it shows "Inherit
device setting" today. Resolution: workspace → project → environment → device.

### 1.5 Multi-device

Environments are facts of one bridge, like projects. Ids are minted per device
(`env-N`), so the SPA keys by `deviceKey(deviceId, environmentId)`. Nothing is
synchronised between devices in this plan; the account-level views merge rows
and disambiguate with the device tag the inbox already uses
(`deviceTagHtml`, `spa/src/core/inboxProjects.js:183`).

### 1.6 Where tasks scope

**Recommendation: tasks scope to the environment.** Reasons:

1. `tracker_tasks.project_key` is already the home path (`schema.rs:226`), and
   every project in an environment shares the environment's home. Two projects
   in one environment would collide on `UNIQUE (project_key, number)` today,
   so the choice is forced: either tasks move up, or diverged projects need a
   new key and their own numbering. The first needs **no migration at all**:
   every existing project becomes the base project of a new environment whose
   home is the same path, so every task keeps its key and number.
2. The user thinks in environments; the implicit project is invisible. A task
   list that forks when someone adds a folder would surprise them.
3. Agents already receive scope from their binding. The binding resolves
   project → environment for task tools; nothing in `mcp.rs` changes shape.

Rules replacing Tasks Spec 1567–1582 (amend the spec in Stage 2):

- A task belongs to exactly one environment and never moves.
- `tasks.list`, `tasks.counts`, `tasks.filterChoices`, `tasks.lookup` and
  `tasks.create` accept `environment_id`; `project_id` stays accepted and is
  resolved to the project's environment (old clients keep working).
- `links.workspace_ids` may name any workspace of any project in the task's
  environment (`refuse_foreign_link`, `bridge/src/app/tracker/mod.rs:~640`,
  compares environments instead of projects). Parent/child tasks: same
  environment. `assignee: project_agent` means the environment's base
  project's agent unless a `project_id` is given.
- Task numbers are per environment, continuing each environment's existing
  sequence (which is the old project's sequence; nothing renumbers).

The alternative, tasks per project with a new `project_key = proj-N`, is a
real migration of every `tracker_tasks` row plus every `project_key` reader
(`store/tracker.rs:113`, `reviews.rs:235`, `delivery_intents.rs:155,188`,
`waiting_on.rs:188`) and leaves the user with two task lists for one codebase.
Not recommended; recorded in 5.1.

---

## 2. Wire and storage changes

### 2.1 Versioning and capability gating

Bump `API_VERSION` once per stage that changes the wire: `3.37.0` (Stage 1),
`3.38.0` (Stage 2); no gaps (memory: wire minors never skip). Each bump also
updates `fixtures/api/versions.json` "current", `fixtures/api/v1/session.hello.json`
and `node scripts/api-verbs-manifest.mjs`, and every new verb needs
`fixtures/api/v1/<verb>.json` (`bridge/tests/api_contract.rs:332`), an effect
class in `bridge/src/api/effects.rs`, a row in `USER_ACTIVITY_VERBS`
(`bridge/src/app/rpc.rs:49`) when it is a user write, and an HTTP route row in
`bridge/src/http/routes/resources.tsv` with the manifests regenerated by
`scripts/http-resource-manifest.mjs`; every new param on an existing verb
needs its fixture updated because `params.strict` refuses undeclared params
(`api_contract.rs:972`).

`FEATURE_CAPABILITIES` (`bridge/src/api/mod.rs:106`) gains:

| capability | since | gates |
| --- | --- | --- |
| `environments` | 3.37.0 | `environment.*` verbs, `environment_id`/`diverged` on project rows, `environments` in `board.list` |
| `environments.createWorkspace` | 3.37.0 | `environment.create_workspace` |
| `changes.environments` | 3.37.0 | `environment` scope on the change bus |
| `secrets.environmentScope` | 3.38.0 | `GrantScope::Environment` on `secrets.create/list/revoke/audit` |
| `tasks.environmentScope` | 3.38.0 | `environment_id` on `tasks.*` list/create verbs |

SPA side: a persisted per-device support record `environment-support`
written at greeting (pattern: `spa/src/core/sourceEditSupport.js:15-34`,
`rememberSourceEditSupport` called from `publishGreeting`,
`spa/src/core/changeEvents.js:1017-1050`); `capabilitiesOf()`
(`spa/src/core/bridgeApi/v1/index.js:157`) gains `environments: {list, create,
createWorkspace, secrets, tasks}`; `NO_CAPABILITIES` (`changeEvents.js:123`)
gains the same shape with every flag false. Views read the record from cache,
never the greeting.

**Old SPA, new bridge.** `project.list`, `project.create`, `workspace.create`
keep every field and every param. `project.create` without `environment_id`
creates an environment implicitly (name = project name, sources = the given
sources) and the base project inside it. The old "Add project" sheet keeps
working and produces a correct model.

**New SPA, old bridge.** Without `environments` the wizard lists projects as
their own environments (one project = one environment, no "new environment"
page; the "Add project" sheet is offered instead), the environments settings
page shows the standard "Update Build on this device to manage environments
here" notice (wording pattern: `spa/src/sheets/projectSources.js:87-89`),
and the switcher/top nav render from `projects` and `workspaces` exactly as
today. Nothing new is painted from connection state.

### 2.2 New and changed RPC verbs

New family file `bridge/src/api/v1/environment.rs` registered with
`v1_method!`:

| verb | params | result | notes |
| --- | --- | --- | --- |
| `environment.list` | `{}` | `{environments: [EnvironmentRow]}` | row: `environment_id, name, home_path, is_git, base_branch, sources[], isolation, isolation_effective, project_ids[], base_project_id?, last_interaction_ms, unread_count` |
| `environment.create` | `{name?, sources: [{path\|remote, name?, base_branch?}], isolation?, parent?}` | `{environment: EnvironmentRow, project: ProjectRow}` | creates the environment **and its base project** in one call (so a fresh environment can take a workspace at once); `parent` as `project.create` has it (`board.rs:206`) |
| `environment.create_workspace` | `{environment_id, name?, isolation?}` | `{workspace, project_id, project_created: bool}` | resolves/creates the base project, then the same path as `workspace.create`; keeps `WorkspaceCreateParams` (`api/v1/workspace.rs:248`) untouched |
| `environment.rename` | `{environment_id, name}` | `{environment}` | renames the environment and its base project's display name; never a folder |
| `environment.delete` | `{environment_id, mode: "keep_folders"}` | `{}` | refused while any project in it has a workspace that is not finished, or any project has added sources, unless the client first deletes them (see 5.4); revokes environment-scoped secrets |
| `environment.add_source` / `remove_source` / `update_source` / `sync_source` | as the `project.*` twins (`board.rs:206-240`) with `environment_id` | as today | forward-looking, same checks as today's project sources |
| `environment.set_isolation` | `{environment_id, isolation}` | `{environment}` | |

Changed:

| verb | change |
| --- | --- |
| `project.create` | new optional `environment_id`; when given, `sources` are **added sources** and the result row has `diverged: true`. Without it: implicit environment as above |
| `project.list` | rows gain `environment_id, environment_name, diverged, added_sources[]`; `sources` keeps listing effective sources (old clients) |
| `project.add_source` / `remove_source` / `update_source` / `sync_source` | operate on the project's **added** sources only; naming an environment source id is refused with "source-N belongs to environment E; edit it there" |
| `project.delete` | unchanged; deleting a base project leaves the environment (a later `environment.create_workspace` recreates it) |
| `workspace.list` / `workspace.get` rows | gain `environment_id` |
| `board.list` | result gains `environments: [EnvironmentRow]` beside `projects` (`fixtures/api/v1/board.list.json` keys `items, projects, runs, …`) |
| `tasks.list/counts/filterChoices/lookup/create` | accept `environment_id` or `project_id` (Stage 2); `TaskView` gains `environment_id` |
| `secrets.create/list/revoke/audit` | `scope` accepts `{environment_id}` (Stage 2) |

MCP tools (`bridge/src/mcp.rs`): no new project arguments anywhere; scope
still comes from the binding. `create_workspace` (`mcp.rs:1546`) stays "cut in
your project". New Project-surface tools `add_environment_source` and
`remove_environment_source` mirror `add_project_source`/`remove_project_source`
(`mcp.rs:1681,1693`) against the agent's project's environment, and
`add_project_source` is reworded to "adds a folder to **this project only**,
making it diverge from its environment". `list_tasks`/`create_task` read and
write the environment's list (1.6); descriptions at `mcp.rs:1112,1145` say so.
Templates to reword: `bridge/templates/project_agent.md` (lines 1, 5, 15),
`notes/task_tools.md`, `notes/workspace.md`, `messages/attached_secrets.md:4`.

### 2.3 Push events

The change bus (`bridge/src/changes.rs`) gains `Scope::Environment` and
`ScopeWire {kind: "environment", id}` beside `board`, `entity`, `all`
(`changes.rs:351-421`). `BoardLists {projects, workspaces, usage_limits}` (`changes.rs:609-614`,
emitted by `note_board_lists_changed`) gains `environments`. Environment list
changes (create, rename, delete, source edits) are published on the **board**
scope so today's `writePushedBoard` (`spa/src/core/cacheSync.js:2068-2091`) learns a third
list, `environments`, with its own stamp. Workspace and project pushes are
unchanged. `secrets.changed` carries the environment scope in Stage 2.

### 2.4 Storage

**`config.json`** (written by `config_value_with_project`,
`bridge/src/app/config/persistence.rs:26-84`):

```jsonc
{
  "next_environment": 3,
  "environments": [
    { "id": "env-1", "name": "build", "next_source": 3,
      "sources": [ { "id": "source-1", "name": "build", "mount": "build", "path": "...", "is_git": true, "base_branch": "main", "sync_base": true },
                   { "id": "source-2", ... } ],
      "isolation": "worktree" }
  ],
  "projects": [
    { "id": "proj-1", "environment_id": "env-1", "added_sources": [], "isolation": null,
      // kept for one release so a rolled-back bridge still loads; removed in Stage 4
      "path": "...", "base_branch": "main", "next_source": 3, "sources": [ ...effective... ] }
  ]
}
```

Loading order: environments first, then projects; a project naming an unknown
`environment_id` is parked (the existing parked-project path,
`persistence.rs:69`) with a reason, never dropped.

**Migration of every existing project** (at first load of a bridge with this
code, before anything is served, inside `AppState` construction):

1. For each project entry without `environment_id`, mint `env-N`, copy
   `name`, `sources`, `next_source`, `isolation` into the environment; set
   `project.environment_id`, `project.added_sources = []`,
   `project.isolation = null` (the override moved up; effective isolation is
   unchanged because the project inherits it back).
2. Parked projects are migrated the same way when unparked.
3. Write the config once, atomically, keeping the legacy project fields.
   Idempotent: a second load finds `environment_id` on every project and does
   nothing. Reversible: a bridge from before this change ignores
   `environments` and reads the legacy fields. (Store Migration Spec: renamed
   not deleted, idempotent, reversible.)
4. Tasks: nothing moves; `project_key` is already the home path (1.6).
5. Secrets: nothing moves; `GrantScope::Project` rows keep working.
6. Workspaces: nothing moves; `.build-workspace.json` is read as today.

Test: a fixture `config.json` with two projects, one parked, one with a
project-level isolation override, three tasks and one project-scoped secret
loads, migrates, serialises, reloads byte-identical, and every task number,
workspace directory and secret remains readable under the same ids.

**SQLite** (`bridge/src/store/schema.rs`, `SCHEMA_VERSION = 25` at
`bridge/src/store.rs:296`): no change in Stage 1. Stage 2 needs **version 26**:
`secret_records.grant_scope` is constrained by
`CHECK (grant_scope IN ('agent','project'))` with a `grant_project_id` column
(`store/secrets/schema.rs:58-62`), so the environment scope is a table rebuild
(new table with `'environment'` allowed and a `grant_environment_id` column,
copy, rename) done in `migrations.rs` with a `pretend_to_be_v25` test like
`migrations.rs:39-154`, and `secret_final_bindings`/`secret_audit`
(`schema.rs:110-141`) gain the environment id beside `project_id`. A store
newer than the binary is refused before anything is written
(`store.rs:369-386`), so the rollback story is "roll the binary forward", as
today. Interaction recency (3.1) is derived, not stored (5.2).

**Path-keyed stores and two projects on one path.** Base and diverged projects
of one environment share the home path. Every path-keyed store in 1.1 must
therefore resolve path → **environment**, with the project named explicitly
where it matters:

- `tracker_tasks.project_key` stays the home path and means the environment
  (1.6).
- `scratch_dir` for the project conversation owner gains the project id in its
  key for diverged projects (`<name>-<sha256(path)[..12]>-<proj-N>`); the base
  project keeps today's key so no folder moves.
- `PersistedRun`/`PersistedPlan` records gain an optional `project_id`;
  recovery (`bridge/src/app/runtime/recovery.rs:333,401`) binds by it when
  present and falls back to the environment's base project when absent, which
  is exactly today's behaviour for every existing row.
- `task_project_maps` becomes path → environment id; the project id is read
  from the task's linked workspace or run when a view needs it.

---

## 3. UX flows

Every screen paints from `build-cache` and `build-ui` first, then pulls
(`ARCHITECTURE.md` "Render from cache", line 4107). The lists below are the
cache records each screen reads; a screen never reads connection state.

### 3.1 Create-workspace wizard

Replaces today's "New workspace in P" modal (`spa/src/core/createWork.js:54-68`)
as the single creation entry point, opened from: the switcher's "New
workspace…" (`toolbar.js:853-869`), the inbox's "New project" button
(`inboxShell.js:358-363`, relabelled "New workspace"), the project view "+"
(`projectView.js:160-180`, preselected to that project), and Ctrl/Cmd-N.

**Page 1, "Where?"** Reads `environments`, `projects`, `workspaces` lists for
every device from the cache plus navigation visits
(`navigationVisits.js`, ≤20 at `uiAddress({view:"navigation", kind:"history",
sub:"visits"})`).

- Search field on top (same fuzzy rank as the switcher,
  `toolbarModel.js:93`).
- **Recent** section: targets interacted with in the last 24 h
  (`RECENT_AFTER_MS`, `inboxOrdering.js:3`), newest first, at most 5. A target
  is an environment or a diverged project. Its interaction time is
  `max(visit time of the target or any of its workspaces, last_activity_ms of
  any of its workspaces, created_at of any of its workspaces)`, all already in
  the cache (`sessionSpans.js:16-24`). Empty: the section is omitted, not
  shown empty.
- **All** section: environments alphabetically, each a row with its identity
  badge (3.4), name, device tag when names clash, and "N workspaces · M
  unread"; diverged projects as indented rows "› Project" under their
  environment. A project of an old bridge (no `environments` capability) is
  listed as itself.
- Last row: **"New environment…"** → page 2.
- Keyboard: ↑/↓/Enter, Esc closes; first row focused when opened from the
  keyboard shortcut.
- Empty (no environments, no projects anywhere): the list is just "New
  environment…" with a line "Build keeps your code in environments. Make one
  from a folder or a Git URL." Offline device: rows still show (cache); the
  device tag reads "Offline" as in the inbox (`OFFLINE_TAG`); choosing one
  proceeds and the pending write surfaces the error on page 3.

**Page 2, "New environment"** (only from the "New environment…" row). The
existing sheet logic (`sheets/newRepo.js:263-283`): device choice when more
than one device, "Folders" (browser via `fs.list`, or Git URL), optional name
that follows the first folder. Draft at `uiAddress({view:"new-environment",
kind:"draft"})` (today `new-project`, `newRepo.js:343-354`). "Back" returns to
page 1 with the draft kept. "Create" calls `environment.create` and goes to
page 3 with the new environment selected. Error: inline under the folder list,
exactly the bridge's refusal text (plain user-facing errors), draft kept.

**Page 3, "Workspace"**: title "New workspace in E" (or "in E › P"), label
input (optional, default `build/<slug>` preview as `branchNamePreview`,
`toolbarModel.js`), isolation select with "Inherit E's setting", and a
read-only list of the directories the workspace will contain (effective
sources). "Create" calls `environment.create_workspace` (or `workspace.create`
for a diverged project / old bridge) through `deviceCall`, upserts the
`workspaces` list (`cacheCreatedWorkspace`, `createWork.js:47-52`) and
navigates to the workspace. Loading: the button shows "Creating…" and the row
appears in the inbox as *provisioning* from the pending write
(`writeOperations.js`). Error: the modal stays open with the refusal under the
button; the pending row is removed.

Mobile (≤760 px): the wizard is a full-height sheet like `.tbmenu`
(`shell.css:882-889`); pages slide, 44 px rows.

### 3.2 Settings: environments and projects

New account route `#/account/environments` (router comment table,
`spa/src/core/router.js:1-15`), linked from the account settings page
(`views/settings.js`) and from the device settings page where "Projects" is
today (`views/deviceSettings.js:46`, `deviceProjects.js:22-29`). It reads
`environments` and `projects` for every device from the cache; per-device
pulls go through `pullDeviceProjection` (pattern `deviceProjects.js:78-90`).

- **List**: one card per environment: badge, name, device tag, home path,
  `N sources · M projects · K workspaces`, "Settings…". Under each, its
  diverged projects as indented rows with "+N added source(s)" and their own
  "Settings…". "New environment…" opens wizard page 2 standalone.
- **Environment settings sheet** (new `sheets/environmentSettings.js`, modelled
  on `projectSettings.js`): General (name, editable → `environment.rename`;
  home path read-only), Sources (today's source cards,
  `sheets/projectSources.js`, against `environment.*_source`), Isolation,
  Secrets (environment-scoped list, Stage 2; the composer from #1006 with scope
  preselected), Projects in this environment (rows → project settings; "New
  project with extra folders…" opens a project sheet that is `project.create
  {environment_id, sources}`), Danger zone (delete: disabled with the reason
  while refused, see 5.4).
- **Project settings sheet** (`projectSettings.js`, amended): General shows
  "In environment E" with a link; Sources splits into "From E" (read-only
  cards with "Edit in E") and "Added here" (editable, `project.*_source`);
  Isolation says "Inherit E's setting"; Danger zone unchanged.
- States: loading shows the cached list immediately and "loading…" only when
  the cache has nothing; error replaces only the still-empty list with the
  refusal text (`deviceProjects.js:87-89`); empty shows "No environments on
  this device yet" with the create button. Old bridge: the page lists its
  projects under a "Projects (this device needs an update to use environments)"
  heading with today's project settings.

### 3.3 Top nav

Today: `tb-switch` left chevron (#618) opens the global picker; the project
crumb; the local workspace selector (`toolbarRender.js:324-364`).

- **Switcher** (chevron, unchanged glyph and target): lists the same targets as
  wizard page 1 (recent on top, then environments with diverged projects
  nested, unread badge per row as today), each row navigates to the
  environment's base project view; footer "New workspace…" opens the wizard.
  Heading "Switch to"; search placeholder "Search environments or workspaces".
  Workspaces keep appearing in search results as today
  (`navigationPickerModel.js:8-14`).
- **Project crumb** becomes the **environment crumb**: badge + name (plus
  "› Project" for a diverged project), as wide as its text (#523 rule).
- **Workspace selector** (next to it, `tb-workspace`): clicking lists **every
  workspace of the project sorted by most recently interacted with**
  (`last_activity_ms` desc, then visit time, then created), the current one
  first and marked, finished ones under a "Finished" fold; footer "New
  workspace in this project" (wizard page 3 with the target fixed) and "Manage
  workspaces". This replaces today's name/status order (`WORKSPACE_STATUS_ORDER`).
  Off a workspace route the selector reads "Workspaces ▾" as today.
- Paint: both menus render from `projects`, `environments`, `workspaces` and
  visits in the cache; unread badges from `navigationUnread`. The 900 px
  pinned-inbox geometry (#631) is untouched: the chevron and crumbs slide
  together; the badge adds no width beyond its 16 px glyph.
- Mobile: unchanged bottom-sheet behaviour (`shell.css:882-889`).

### 3.4 Badges for projects and workspaces

An identity badge, not a count. Deterministic from the key, so every device
and tab paints the same thing without storage: hue = `hash(deviceKey) mod 12`
from a 12-step palette defined as new tokens `--badge-1…12` in both themes
(`styles.css:164-237`; nothing else in the client knows a colour,
`core/theme.js:1-14`), glyph = first two letters of the name. Environments
and diverged projects get a **filled** 16 px rounded square; workspaces get an
**outlined** 16 px circle in their project's hue so a workspace is visibly
"of" its environment. Unread counts keep today's `.badge` pill
(`styles.css:471`) beside it. Both are `aria-hidden`; the row text stays the
accessible name.

Placement: switcher rows and wizard rows; the environment crumb; inbox project
block heads (`projectHeadHtml`, `inboxProjects.js:248-264`) in the grouped
face; inbox rows in the flat face where the project tag is today
(`projectTagHtml`, `inbox.js:964-967`: badge before the tag text); the
workspace selector rows. The inbox status dot (`inboxStatusDot.js`) is
unchanged and sits where it does today.

### 3.5 Inbox

Grouping stays by project key (`workspaceBlockFor`, `inboxProjects.js:78-102`),
but the block head reads the environment name (plus "› Project" when
diverged) with its badge, and blocks of the same environment are adjacent
(sort by environment, then `byAnchor`). The "New project" button
(`inboxShell.js:358-363`) becomes "New workspace" and opens the wizard. Row
"+" on a block head (`data-project-create`) opens wizard page 3 for that
project.

---

## 4. Stages

Each slice is one branch from `dev`, independently mergeable, TDD, with its
own fixture/version updates where it touches the wire. "Bridge" slices run
`cargo test`/clippy/fmt; "SPA" slices run lint/test/build; both run gitleaks
and semgrep per AGENTS.md.

### Stage 1: MVP, fully functional end to end

Goal: a user on a fresh or existing device can create a workspace through the
wizard (existing environment or new one), the model is migrated, the nav shows
environments, and nothing an old client does breaks.

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| 1.1 | **Bridge: environment model + config migration** (`app/environments/`, `Project.environment_id/added_sources`, `config_value_with_project`, load) | every existing project loads into its own environment; config round-trips; second load is a no-op; a pre-change bridge still loads the written file | `config_migration_tests.rs`: fixture with 2 projects (one parked, one isolation override) → environments; idempotence; legacy reload; `next_source` continuity; `sources_for` = env ++ added |
| 1.2 | **Bridge: wire 3.37.0** (`environment.list/create/create_workspace`, `project.list` fields, `board.list.environments`, `environments` capabilities, board-scope push, fixtures, manifest) | all `api_contract.rs` tests pass; `project.create` without `environment_id` makes an implicit environment; `environment.create_workspace` creates the base project once and reuses it after | contract fixtures for 3 new verbs; `implicit_environment_from_legacy_project_create`; `create_workspace_in_environment_reuses_base_project`; push test that an `environment.create` lands as a board change |
| 1.3 | **Bridge: diverged projects** (`project.create {environment_id, sources}`, `project.*_source` on added sources, `diverged` flag, refusal of editing env sources via project verbs, one-base-per-environment rule, project-id-keyed scratch folder and `project_id` on persisted runs/plans with base-project fallback) | adding a folder to a project flips `diverged`; removing the last one is refused if a base exists; env-source ids refused through `project.remove_source`; a diverged project's coordinator and runs recover to that project after a restart | `added_source_makes_project_diverged`; `last_added_source_removal_refused_when_base_exists`; `project_verbs_refuse_environment_source_ids`; overlap refusal across environments; `diverged_project_runs_recover_to_their_project`; `base_project_keeps_legacy_scratch_dir` |
| 1.4 | **SPA: capability + cache plumbing** (`environment-support` record, `capabilitiesOf().environments`, `NO_CAPABILITIES`, `environments` session list, `writePushedBoard` third list, `readSyncLists` asks `environment.list` when supported) | a greeting with the capability writes the record; a board push updates the cached environments list; without the capability nothing new is asked | `environmentSupportWiring.test.js`, `cacheSyncChanges.test.js` additions, `namedCapabilitiesWiring.test.js` |
| 1.5 | **SPA: wizard pages 1 and 3** (`core/createWorkspaceWizard.js`, target model `environmentTargets()` with recents, replaces `createWork.js` modal; every caller switched) | opens from switcher, inbox, project view, Ctrl/Cmd-N; recents last 24 h on top; diverged projects nested; old bridge lists projects; create lands and navigates; offline device shows the pending row then the error | `createWorkspaceWizardModel.test.js` (recents, nesting, fallback), `createWorkspaceWizardDom.test.js` (pages, keyboard, error, pending row), `createWork.test.js` retargeted |
| 1.6 | **SPA: wizard page 2** (`newRepo.js` reshaped into `environment.create`, draft address renamed, Back keeps the draft) | a folder or URL creates an environment and the wizard continues to page 3 with it selected; old bridge falls back to `project.create` | `newProjectDom.test.js` → `newEnvironmentDom.test.js`; draft persistence test |
| 1.7 | **SPA: top nav** (switcher lists environments, environment crumb, workspace selector by recency, "New workspace in this project") | rows sorted by `last_activity_ms` desc then visit; current first; finished folded; #523/#618/#631 geometry tests still pass | `toolbarModel.test.js` (new `workspaceMenuModel` order), `toolbarDom.test.js`, `navigationPicker.test.js`, `test/browser/pinnedInboxToolbarLayout.test.js` unchanged |
| 1.8 | **SPA: settings page MVP** (`#/account/environments`, list, environment settings sheet with General + Sources + Projects, project sheet amendments) | every existing project appears under its environment; sources edit via `environment.*_source`; project sheet shows inherited vs added; old-bridge notice | `environmentsSettingsDom.test.js`, `projectSettings.test.js` amendments, router test for the new route |
| 1.9 | **Docs**: `ARCHITECTURE.md` (Workspaces and worktrees 1636, RPC 163, Capability gating 5000), `docs/using.md`, this plan's status | sections describe the code as merged | n/a (docs) |

Done when: on dev, a user creates a workspace in a brand-new environment from
the inbox in three clicks, an old SPA tab still lists and creates projects, and
`project.list` for a migrated device shows every project with `environment_id`.

### Stage 2: full feature

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| 2.1 | **Bridge: environment secrets** (`GrantScope::Environment`, store schema 26 table rebuild, membership check, revoke on environment delete, name override resolution, `secrets.*` scope param, `secrets.changed` scope, `secrets.environmentScope` capability, 3.38.0) | an environment secret is readable by agents of every project in it and by none outside; project secret of the same name wins; deletion revokes | `environment_scoped_secret_readable_across_projects`; `project_secret_overrides_environment_name`; `environment_delete_revokes_environment_scope` (twin of `project_delete_entry_point_revokes_project_scoped_secret`); leak sweep row 14 rerun |
| 2.2 | **Bridge: tasks scope to environment** (`environment_id` on tasks verbs, `TaskView.environment_id`, `refuse_foreign_link` by environment, parent rule, `tasks.environmentScope`; Tasks Spec amended) | a diverged project's agent lists the environment's tasks; links to another project's workspace in the same environment are accepted; cross-environment refused | `tasks_list_by_environment_equals_by_project`; `link_workspace_of_sibling_project_accepted`; `link_workspace_of_other_environment_refused`; MCP `list_tasks` from a diverged project worker |
| 2.3 | **Bridge: environment rename/delete/isolation** (`environment.rename/delete/set_isolation`, refusals, parked-project unpark migration) | rename changes display only; delete refused with the exact reason while projects have live workspaces or added sources; isolation resolves workspace → project → environment → device | `environment_rename_keeps_ids_and_paths`; `environment_delete_refused_with_live_workspace`; `isolation_resolution_order` |
| 2.4 | **Bridge: MCP** (`add_environment_source`/`remove_environment_source`, reworded `add_project_source`, `create_workspace` unchanged, templates) | project agent can add a folder to its environment; descriptions mention environments; a Coding-surface agent still has no such tool | `mcp_project_surface_lists_environment_source_tools`; template render snapshot |
| 2.5 | **SPA: secrets in environment settings + composer scope** (#1006 composer gains "This environment" scope; chips show "overrides environment") | create/list/revoke env secrets from the sheet; old bridge hides the scope | `secretsWire.test.js`, `environmentSettingsSecrets.test.js` |
| 2.6 | **SPA: tasks by environment** (project view Tasks tab reads by `environment_id` when supported; task rows show the linked workspace's project when diverged) | one task list per environment across its projects | `trackerPagesWiring.test.js` additions |
| 2.7 | **SPA: environment rename/delete/isolation in settings**; project sheet "New project with extra folders…" | reasons for refused delete shown verbatim and the button disabled; rename repaints every crumb and block head from the cache | `environmentsSettingsDom.test.js` additions |
| 2.8 | **SPA: inbox** (block heads by environment, adjacency sort, "New workspace", block "+") | grouped face shows environment names and badges; sort stable | `inboxProjects.test.js`, `inboxDom.test.js` |
| 2.9 | **SPA: badges** (tokens, `identityBadgeHtml`, placements of 3.4) | identical hue across tabs for one key; both themes pass contrast on the badge glyph | `identityBadge.test.js`, theme token test |

### Stage 3: polish

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| 3.1 | Wizard recents across devices with device tags; keyboard-only path; mobile sheet motion; reduced-motion | a11y audit of the dialog passes; ≤760 px layout screenshot test | browser layout test |
| 3.2 | Wizard: optional "Add a folder to this workspace's project" on page 3 that creates a diverged project inline (needs a project name field; see 5.5) | creating with an extra folder yields `diverged: true` and a named project | wizard model + DOM tests |
| 3.3 | Workspace selector: "Finished" fold, counts, empty copy ("No workspaces in E yet") | | `toolbarDom.test.js` |
| 3.4 | Environment view (`#/environment/<id>` → base project view with an "Environment" tab listing projects and sources); redirects keep `#/project/<id>` working | | router + view tests |
| 3.5 | Discoverability: first-run hint on the wizard ("A workspace is a copy of your environment's folders you can work in"), empty-state copy on the inbox | copy approved by Zech | DOM tests |
| 3.6 | `docs/using.md` and `ARCHITECTURE.md` terminology pass; Terminology Audit amended with retained contracts (RPC names unchanged, routes unchanged) | | n/a |

### Stage 4: cleanup of old systems

Safe once Stage 2 has been live on `main` for one release and the SPA's
`SPA_API_RANGE` floor (`spa/src/core/bridgeApi/index.js`) is raised to
`>=3.38.0`.

| # | what goes | when safe | test |
| --- | --- | --- | --- |
| 4.1 | Legacy project fields in `config.json` (`path`, `base_branch`, `next_source`, `sources` on project entries); `Project.repo_path/base_branch/next_source` fields replaced by accessors on the environment | after one release with 4.0-free rollback no longer needed (Zech's call); loader keeps reading them for one more release, writer stops | migration test drops the legacy fields and reloads |
| 4.2 | `createWork.js` modal and `CREATE_KINDS`; `newRepo.js` project path; "Add project" buttons | once every caller uses the wizard (end of Stage 1) | remove tests |
| 4.3 | `project.create` without `environment_id` (implicit environment) → refused with "name an environment" | when `SPA_API_RANGE` floor ≥ 3.37.0 and the desktop app ships that SPA | contract fixture flips to the refusal |
| 4.4 | `WORKSPACE_STATUS_ORDER` name/status sort in `toolbarModel.js` | end of Stage 1 | |
| 4.5 | Tasks Spec 1567–1582 old wording; `project_key` comment at `schema.rs:226` reworded to "environment home path" | Stage 2 merge | |
| 4.6 | `project.*_source` verbs on base projects (they can only ever add; keep them for diverged projects) | not removed; documented | |

---

## 5. Oversights and open questions

Each with a recommended answer. Items marked **(Zech)** need his call before
the slice that depends on them starts.

1. **Task scoping across projects in one environment.** Forced by
   `project_key` being the home path (1.6). *Recommend*: tasks scope to the
   environment; no migration; `tasks.*` accept `environment_id`; links allowed
   across sibling projects. **(Zech)** since it amends the Tasks Spec rule.
2. **What "recent" counts as interaction.** *Recommend*: for the wizard and
   switcher, `max(visit to the target or a workspace in it, workspace
   last_activity_ms, workspace created)` within 24 h, computed in the SPA
   from cache; no new bridge storage. Agent-only activity (a worker posting at
   3 am) counts because `last_activity_ms` is conversation activity
   (`workspaces/mod.rs:433`), which is what you want to see first in the
   morning. Visits are per tab/device profile, so recents differ slightly per
   device; acceptable.
3. **Environment rename.** *Recommend*: display-only; never moves folders; base
   project display name follows; ids, task keys, workspace roots unchanged.
   The folder's name no longer binds the environment's name after the first
   edit (today the project name is read-only and follows the folder).
4. **Environment delete with live projects.** *Recommend*: refuse while any
   project has an unfinished workspace or added sources; the SPA shows the
   reason and offers "Finish workspaces" / "Delete project". Deleting removes
   the registration only (folders stay, as `project.delete` today), revokes
   environment secrets, and leaves tasks in the store under the home path so a
   re-added environment at the same path sees them again (today's behaviour
   for projects; confirm this is wanted). **(Zech)**
5. **Diverged projects need a name.** A base project's name is the
   environment's; a diverged project is "E › ?" The wizard's MVP creates no
   diverged projects (settings or MCP do); Stage 3.2 adds an inline path.
   *Recommend*: default the name to the first added folder's name, editable.
6. **Secrets security.** Checklist rows 8 and 11 generalise cleanly; the new
   risk is **widening**: a project-scoped secret must not become readable by a
   sibling project by any path (attachment resolution, CLI release,
   `attached_secrets.md` rendering). *Recommend*: a negative test per
   mechanism (release authority, list, attach, audit) proving a sibling
   project cannot read a project-scoped secret, plus the override rule being
   project-wins only. Moving an existing project-scoped secret "up" to the
   environment is **not offered** (re-create instead) so no value is ever
   re-sealed by the bridge.
7. **Sources added to project vs environment.** *Recommend*: the UI defaults
   everything to the environment; "add to this project only" is a secondary
   action in project settings and the MCP `add_project_source`. The agent
   tool description says what it does to divergence so a project agent does
   not fork its project by accident.
8. **Multi-device and E2EE.** Environments are per device, like projects; no
   cross-device identity. Same name on two devices is two environments with
   device tags. Nothing new crosses the relay in plaintext: environment names
   and paths travel inside the same sealed channel as project rows today.
   Open: should the account later sync "environment of the same remote URL"
   across devices? *Recommend*: out of scope; note in the Scope doc.
9. **MCP tools.** `create_workspace` stays in the agent's project (no
   environment argument; the binding decides). Add
   `add_environment_source`/`remove_environment_source` on the Project
   surface. *Recommend* no `create_project` tool: agents should not create
   diverged projects. `list_projects` (router) lists environments and nests
   diverged projects.
10. **Inbox grouping.** *Recommend*: keep blocks per project (ordering and
    unread maths unchanged), head text and badge by environment, adjacency
    sort. A single block per environment would hide which project a diverged
    workspace belongs to.
11. **Mobile.** The wizard is a bottom sheet; pages slide; the workspace
    selector's recency list is the same sheet as today's menu. Open: page 2's
    folder browser on a phone is already awkward (`sheets/browser.js`);
    *recommend* the Git URL path is first on mobile.
12. **Discoverability of the implicit project.** The word "project" nearly
    disappears from the UI (environment crumb, wizard, settings). *Recommend*
    the user-facing vocabulary be **environment** and **workspace**, with
    "project" appearing only for diverged ones ("E › P") and in project
    settings. **(Zech)**: confirm the vocabulary before 1.5/1.7 copy is
    written; the Terminology Audit (#537) is the place to record it.
13. **Badges: identity or counts?** The ask says "badges for projects and
    workspaces". *Recommend* identity badges (3.4) beside the existing unread
    pills; counts already exist. **(Zech)**
14. **`project.create` compatibility semantics.** An old SPA's "Add project"
    makes an implicit environment; two old-client adds of folders already in an
    environment are refused by overlap as today. Fine, but the old client's
    project settings then shows environment sources as editable through
    `project.update_source`, which the new bridge must still accept for a
    **base** project (it edits the environment source). *Recommend*: accept,
    route to the environment, and only refuse for diverged projects.
15. **One base project per environment.** Needed for "create in E" to be
    unambiguous. Edge: a user deletes the base project while a diverged one
    remains; the next `environment.create_workspace` recreates a base. OK.
16. **Compaction policy and project agents** stay per project (unchanged);
    the environment has none. Open whether a project agent should exist per
    environment instead. *Recommend*: leave as is in this plan.
17. **"Environment" already means dev/prod in secrets.** `SecretEnvironment
    {Dev, Prod}` (`bridge/src/thread/secrets.rs:14-33`) is what `buildctl --env`
    and the secret runtime CLI mean by the word. Two meanings of "environment"
    in one codebase, one of them in a security-sensitive CLI, is a trap.
    *Recommend*: rename the secrets channel to **`stage`** (`SecretScope.stage`
    already exists, `api/v1/secrets.rs:21-25`) in a mechanical slice before
    1.1, or pick another user-facing word for this feature (**"space"**,
    **"codebase"**). **(Zech)**: the plan uses "environment" throughout until
    told otherwise.
18. **Two projects on one path.** Base and diverged projects share the home
    path, and runs, plans, the conversation scratch folder and the task
    path→id map are keyed by path (1.1, 2.4). Without the explicit `project_id`
    in 2.4, a diverged project's coordinator would reuse the base project's
    scratch folder and recovery would bind its runs to the wrong project.
    Covered by slice 1.3; recorded here because it is easy to miss.
19. **Secrets schema rebuild.** The grant scope CHECK constraint makes the
    environment scope a table copy, not an `ALTER`. *Recommend* it stays in
    Stage 2 (2.1) with the leak sweep rerun, and that Stage 1 ships no secrets
    change at all.
20. **Wire naming.** `environment.create_workspace` versus a new optional
    `environment_id` on `workspace.create`. *Recommend* the new verb: it keeps
    the existing fixture and `params.strict` shape untouched and makes the
    implicit project creation a named, auditable operation.

---

## 6. Risks

- **Scope creep into secrets and tasks.** Both are 100/100 checklists today.
  Mitigation: Stage 1 touches neither; Stage 2 slices 2.1 and 2.2 each carry
  their own checklist rows and negative controls per mechanism.
- **Config migration on a live device.** A bug here parks or drops projects.
  Mitigation: migrate before serving, keep legacy fields, byte-identical
  reload test, parked never dropped, rollback path proven in the test.
- **Two id namespaces per project** (environment sources and added sources)
  confusing readers. Mitigation: one counter on the environment; added
  sources are plain `source-N` from it.
- **Old SPA on new bridge** silently creating environments the user did not
  name. Mitigation: implicit environment takes the project's name; the next
  settings visit on a new SPA shows it; nothing is lost.
- **Recents differing per device/tab** (visits are local). Accepted; stated in
  5.2.
- **Nav geometry regressions** (#523/#618/#631 have browser layout tests).
  Mitigation: badges are fixed 16 px; the existing layout tests stay green as
  acceptance for 1.7.
- **Word collision with the secrets dev/prod channel** (5.17). Mitigation:
  decide the word before any copy or verb name lands.
- **Vocabulary churn.** If Zech wants "project" to stay the user-facing word,
  copy in 1.5/1.7/1.8 changes but no model does. Asked in 5.12.

---

## 7. Security checklist delta

New rows for `Project Sources Security Checklist.md` (Stage 1) and
`Secrets Security Checklist.md` (Stage 2), each with a named test:

| checklist | control | test |
| --- | --- | --- |
| Project Sources | Overlap refusal spans every environment and every project's added sources on the device; a folder held by environment A is refused as an added source of a project in B and vice versa | `source_overlap_refused_across_environments` |
| Project Sources | `project.*_source` verbs cannot edit or remove an environment source through a diverged project; the refusal names the environment | `project_verbs_refuse_environment_source_ids` |
| Project Sources | Config migration writes nothing until every project is assigned; a failure leaves the old file intact (atomic temp + rename, `persistence.rs:86-104`) | `migration_failure_leaves_config_untouched` |
| Project Sources | Environment rename is bounded one-line text with no control characters and never a path | `environment_rename_is_one_bounded_line` |
| Secrets (8) | Environment scope derives from the issued session's project's environment at read time, never from a selector; a project moved out of an environment (impossible today; guard anyway) loses access | `environment_membership_revalidated_per_read` |
| Secrets (11) | Environment deletion revokes environment-scoped secrets before the registration is removed; project deletion leaves environment scope intact | `environment_delete_revokes_environment_scope` |
| Secrets (new) | A project-scoped secret is never released to a sibling project's agent by list, attach, CLI release or audit | one negative test per mechanism |
| Secrets (19) | `secrets.environmentScope` capability, fixtures, manifest and version pins; an older client never sees the scope | `secrets_contract_includes_environment_scope` |
| Tasks | A task link to a workspace of another environment is refused; a sibling project's workspace is accepted | `link_workspace_of_other_environment_refused` |
| Navigation (#523) | Badges add no interactive element and no accessible name; layout tests at 900 px unchanged | `pinnedInboxToolbarLayout.test.js` |

Rows touched but unchanged in meaning: Secrets 13–16 (values never in SPA
persistence or history; the environment sheet renders metadata only).
