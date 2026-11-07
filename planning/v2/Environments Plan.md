# Environments: a layer above projects

Planning for **#1028**, 2026-11-07. **Planning only; nothing here is
implemented.** Code inspected at `f1376d41` (`origin/dev`). Wire today is
`3.36.0` (`bridge/src/api/mod.rs:78`).

Companions: `Project Workspace Spec.md` (#522), `Navigation Spec.md` (#523,
#618, #631), `Project Workspace Terminology Audit.md` (#537), `workspaces.md`,
`Store Migration Spec.md`, `Project Sources Security Checklist.md`,
`Secrets Security Checklist.md`, `Tasks Spec.md` (scope rules, lines 1567–1582).

## The model in three lines

- **Environment** = the source directories and the secrets. It is where the
  code lives. It has no tasks, no agents and no workspaces of its own.
- **Project** = **one line of work** inside an environment, with **its own
  task board**, its own project agent and workers, and optionally folders of
  its own. Many projects share one environment; most add nothing to it and
  differ only in what they are working on. Separating their tasks is the
  reason for this change.
- **Workspace** = unchanged. A checkout of its project's effective sources
  (environment sources plus the project's added folders), cut as today.

The user creates a **workspace** through a wizard. Picking an environment there
creates a **new project** in it (a fresh task board) and the first workspace of
that project in one step; the project is implicit. "New workspace in this
project" from the top nav adds a second workspace to the **same** line of work
and the same task board. A project with folders of its own ("diverged") is
listed in the wizard as its own environment, as Zech asked.

Zech's ask, in one paragraph: a layer above projects, **environments**, owns
source directories and secrets. Projects are created from environments and may
add source directories; workspaces and projects are otherwise unchanged. The
user creates workspaces, not projects, through a wizard that picks an
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
- **Tasks** belong to exactly one project and never move (Tasks Spec
  1567–1582). They are keyed by the project's canonical home **path**, not
  `proj-N`: `tracker_tasks.project_key` with `UNIQUE (project_key, number)`
  (`bridge/src/store/schema.rs:226-245`); `tracker_project_path` resolves it
  from `project.repo_path` (`bridge/src/app/tracker/mod.rs:719`). The path was
  chosen so a project deleted and re-added at the same folder finds its tasks
  again. It works today only because overlap refusal guarantees no two
  projects share a path.
- Several other stores are keyed by the project's **path** for the same
  reason: persisted runs and plans (`PersistedRun.project_path`,
  `PersistedPlan.project_path`, `bridge/src/store/entities.rs:40-47,99-112`),
  the project conversation owner's scratch folder (`scratch_dir`,
  `bridge/src/app/projects/conversation.rs:27-41`), and the path→id map
  `task_project_maps` (`bridge/src/app/tracker/read.rs:39-50`). Secrets,
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
    │                                  default isolation
    ├─ project    proj-A        owns: environment_id, project_key, tasks (own numbering),
    │   │                              added_sources[] (empty), isolation override, compaction,
    │   │                              project agent + workers, orchestrator
    │   ├─ workspace ws-…       owns: project_id, directories (one per effective source),
    │   │                              isolation, name, lock          (unchanged)
    │   └─ workspace ws-…
    ├─ project    proj-B        another line of work on the same code, its own tasks
    └─ project    proj-C        diverged: added_sources non-empty; shown as its own
                                environment in the wizard
```

**Environment** (new, `bridge/src/app/environments/`): `{id: env-N, name,
sources: Vec<ProjectSource>, next_source, isolation: Option<Isolation>}`. It
owns the source directories, the one `source-N` counter, and the scope of
environment secrets. Its first source is its **home** (the folder its name
follows by default). Overlap refusal (a path that is, contains, or sits inside
another registered source; a remote already registered; a path inside the
secrets vault; `source_request`, `bridge/src/app/projects/requests.rs:285-365`)
moves up to the environment and applies across every environment and every
project's added sources on the device, with one relaxation for sibling
projects (below).

**Project**: keeps `id`, `name`, `orch`, `isolation`, compaction, agents, and
gains `environment_id: String`, `project_key: String` (1.6) and
`added_sources: Vec<ProjectSource>`. Its **effective sources** are
`environment.sources ++ project.added_sources`, in that order, and that is
what `sources_for` returns. `repo_path`/`base_branch` become derived from the
environment home (kept as fields for one release for the code that reads them,
then removed in Stage 4). Added source ids are minted from the
**environment's** counter so a workspace's `directories[].source_id` is unique
within the environment and a removed id is never reissued anywhere in it.
`name` becomes editable (it no longer has to follow a folder); it is what the
inbox block head, the crumb and the task board are titled.

**Many projects per environment is the normal case.** Nothing distinguishes two
projects with no added folders except their tasks, agents and workspaces. There
is no "base" or canonical project; "create a workspace in E" always creates a
**new project** in E (2.2 `environment.create_project`). A second workspace in
an existing line of work comes from "New workspace in this project" (3.3),
which is today's `workspace.create`.

**Diverged project.** A project **diverges from its environment iff
`added_sources` is non-empty**. Nothing else counts: an isolation override, a
compaction override, a project agent roster, or project-scoped secrets do not
make a project its own thing in the wizard, because none of them changes what
a workspace cut from it contains, and a new project created from the wizard
never inherits any of them (project secrets belong to that line of work, 1.3).
Detection is on the bridge: `project.diverged = !project.added_sources.is_empty()`,
carried on the wire as `diverged: bool` on every project row, never computed
client-side from sources. A diverged project is listed in the wizard as
"Environment › Project" and acts as a **template**: picking it creates a new
project in the same environment whose `added_sources` are copies of the
template's (same paths, new ids from the environment counter). To allow that,
overlap refusal accepts a path that is **exactly** equal to an added source of
another project in the **same** environment (containment and remotes are still
refused as today; cross-environment duplicates are still refused). Removing a
project's last added folder simply makes it a non-diverged project.

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
  deleted (row 11 analogue). Agent and Project scopes are unchanged; a
  project-scoped secret stays private to its line of work and is never copied
  to a project created from the wizard.
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

### 1.6 Tasks stay on the project

Tasks belong to the project, numbered per project, exactly as the Tasks Spec
says today. Two projects in one environment have two independent boards, each
starting at #1. The only change is the **key**, because the home path no
longer identifies a project:

- Each project gets a stable **`project_key`** string persisted in
  `config.json`. For every **existing** project the migration sets it to the
  canonical home path it already uses, so not one `tracker_tasks` row changes.
  For every **new** project it is `project:<uuid>` minted at creation (never
  derived from `proj-N`, so it is never reissued even if a counter is).
- `tracker_project_path` (`tracker/mod.rs:719`) returns `project.project_key`
  instead of the canonical path; `task_project_maps` maps key → project id
  (one-to-one by construction). Every `project_key` reader
  (`store/tracker.rs:113`, `reviews.rs:235`, `delivery_intents.rs:155,188`,
  `waiting_on.rs:188`) is untouched: the column keeps its name and type.
- The same key replaces the path for the other path-keyed stores (2.4), so a
  new project's scratch folder, runs and plans are its own.
- `tasks.*` verbs keep `project_id`; nothing on the task wire changes.
  `refuse_foreign_link` (`tracker/mod.rs:~640`) keeps comparing projects: a
  task may link only its own project's workspaces. MCP `list_tasks`/`create_task`
  are unchanged (`mcp.rs:1112,1145`).
- Consequence for delete: a deleted project's key is never seen again, so
  "re-add the folder and find the tasks" stops being a thing for projects
  created after this change (5.2).

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
| `environments` | 3.37.0 | `environment.*` verbs, `environment_id`/`diverged`/`added_sources` on project rows, `environments` in `board.list` |
| `environments.createProject` | 3.37.0 | `environment.create_project` (the wizard's one call) |
| `changes.environments` | 3.37.0 | `environment` scope on the change bus |
| `projects.rename` | 3.37.0 | `project.rename` |
| `secrets.environmentScope` | 3.38.0 | `GrantScope::Environment` on `secrets.create/list/revoke/audit` |

SPA side: a persisted per-device support record `environment-support`
written at greeting (pattern: `spa/src/core/sourceEditSupport.js:15-34`,
`rememberSourceEditSupport` called from `publishGreeting`,
`spa/src/core/changeEvents.js:1017-1050`); `capabilitiesOf()`
(`spa/src/core/bridgeApi/v1/index.js:157`) gains `environments: {list, create,
createProject, secrets}` and `projects.rename`; `NO_CAPABILITIES`
(`changeEvents.js:123`) gains the same shape with every flag false. Views read
the record from cache, never the greeting.

**Old SPA, new bridge.** `project.list`, `project.create`, `workspace.create`
keep every field and every param. `project.create` without `environment_id`
creates an environment implicitly (name = project name, sources = the given
sources) and one project inside it. The old "Add project" sheet keeps working
and produces a correct model.

**New SPA, old bridge.** Without `environments` the wizard lists projects as
their own environments (one project = one environment, no "new environment"
page; the "Add project" sheet is offered instead) and picking one creates a
workspace in that project (today's `workspace.create`), the environments
settings page shows the standard "Update Build on this device to manage
environments here" notice (wording pattern: `spa/src/sheets/projectSources.js:87-89`),
and the switcher/top nav render from `projects` and `workspaces` exactly as
today. Nothing new is painted from connection state.

### 2.2 New and changed RPC verbs

New family file `bridge/src/api/v1/environment.rs` registered with
`v1_method!`:

| verb | params | result | notes |
| --- | --- | --- | --- |
| `environment.list` | `{}` | `{environments: [EnvironmentRow]}` | row: `environment_id, name, home_path, is_git, base_branch, sources[], isolation, isolation_effective, project_ids[], unread_count` |
| `environment.create` | `{name?, sources: [{path\|remote, name?, base_branch?}], isolation?, parent?}` | `{environment: EnvironmentRow}` | creates the environment only; `parent` as `project.create` has it (`board.rs:206`). The wizard follows it with `environment.create_project` |
| `environment.create_project` | `{environment_id, name, from_project_id?, workspace?: {name?, isolation?}}` | `{project: ProjectRow, workspace?: WorkspaceRow}` | the wizard's call: mints the project (`project_key = project:<uuid>`, `added_sources` copied from `from_project_id` when it names a diverged project of the same environment), then cuts the first workspace by the same path as `workspace.create` (`WorkspaceCreateParams`, `api/v1/workspace.rs:248`, untouched). One effect, one audit line |
| `environment.rename` | `{environment_id, name}` | `{environment}` | display name only; never a folder |
| `environment.delete` | `{environment_id}` | `{}` | refused while any project belongs to it (delete the projects first, 5.2); revokes environment-scoped secrets |
| `environment.add_source` / `remove_source` / `update_source` / `sync_source` | as the `project.*` twins (`board.rs:206-240`) with `environment_id` | as today | forward-looking, same checks as today's project sources |
| `environment.set_isolation` | `{environment_id, isolation}` | `{environment}` | |

Changed:

| verb | change |
| --- | --- |
| `project.create` | new optional `environment_id`; when given, `sources` are **added sources** and the result row has `diverged: true`. Without it: implicit environment as above |
| `project.rename` (new, `project.rs`) | `{project_id, name}`; the name was folder-bound and read-only until now |
| `project.list` | rows gain `environment_id, environment_name, diverged, added_sources[]`; `sources` keeps listing effective sources (old clients) |
| `project.add_source` / `remove_source` / `update_source` / `sync_source` | operate on the project's **added** sources only; naming an environment source id is refused with "source-N belongs to environment E; edit it there". Exception for old clients: `project.update_source`/`sync_source` on an environment source id from a project with no added sources is routed to the environment (5.9) |
| `project.delete` | gains `tasks: "delete"` (required once `projects.deleteTasks` is advertised, Stage 2); refused while the project has an unfinished workspace; removes the project's tasks with it (5.2) |
| `workspace.list` / `workspace.get` rows | gain `environment_id` |
| `board.list` | result gains `environments: [EnvironmentRow]` beside `projects` (`fixtures/api/v1/board.list.json` keys `items, projects, runs, …`) |
| `tasks.*` | **unchanged** |
| `secrets.create/list/revoke/audit` | `scope` accepts `{environment_id}` (Stage 2) |

MCP tools (`bridge/src/mcp.rs`): no new project arguments anywhere; scope
still comes from the binding. `create_workspace` (`mcp.rs:1546`) stays "cut in
your project", `list_tasks`/`create_task` stay "your project's tasks". New
Project-surface tools `add_environment_source` and `remove_environment_source`
mirror `add_project_source`/`remove_project_source` (`mcp.rs:1681,1693`)
against the agent's project's environment, and `add_project_source` is
reworded to "adds a folder to **this project only**, making it diverge from
its environment". No tool creates projects. Templates to reword:
`bridge/templates/project_agent.md` (lines 1, 5, 15), `notes/task_tools.md`,
`notes/workspace.md`, `messages/attached_secrets.md:4`.

### 2.3 Push events

The change bus (`bridge/src/changes.rs`) gains `Scope::Environment` and
`ScopeWire {kind: "environment", id}` beside `board`, `entity`, `all`
(`changes.rs:351-421`). `BoardLists {projects, workspaces, usage_limits}`
(`changes.rs:609-614`, emitted by `note_board_lists_changed`) gains
`environments`. Environment list changes (create, rename, delete, source
edits) are published on the **board** scope so today's `writePushedBoard`
(`spa/src/core/cacheSync.js:2068-2091`) learns a third list, `environments`,
with its own stamp. `environment.create_project` publishes the new project and
workspace through the existing project/workspace board pushes, so an old SPA
tab sees both. `secrets.changed` carries the environment scope in Stage 2.

### 2.4 Storage

**`config.json`** (written by `config_value_with_project`,
`bridge/src/app/config/persistence.rs:26-84`):

```jsonc
{
  "next_environment": 3,
  "environments": [
    { "id": "env-1", "name": "build", "next_source": 4,
      "sources": [ { "id": "source-1", "name": "build", "mount": "build", "path": "...", "is_git": true, "base_branch": "main", "sync_base": true },
                   { "id": "source-2", ... } ],
      "isolation": "worktree" }
  ],
  "projects": [
    { "id": "proj-1", "environment_id": "env-1", "name": "build",
      "project_key": "/home/zech/Projects/build",          // migrated: the old path key
      "added_sources": [], "isolation": null,
      // kept for one release so a rolled-back bridge still loads; removed in Stage 4
      "path": "...", "base_branch": "main", "next_source": 3, "sources": [ ...effective... ] },
    { "id": "proj-7", "environment_id": "env-1", "name": "Environments plan",
      "project_key": "project:6f1c…",                      // new: never a path
      "added_sources": [ { "id": "source-3", ... } ], "isolation": null }
  ]
}
```

Loading order: environments first, then projects; a project naming an unknown
`environment_id` is parked (the existing parked-project path,
`persistence.rs:69`) with a reason, never dropped.

**Migration of every existing project** (at first load of a bridge with this
code, before anything is served, inside `AppState` construction). Each
existing project becomes **one project under one new environment derived from
its sources**; nothing is merged, because overlap refusal guarantees no two
existing projects share a folder:

1. For each project entry without `environment_id`, mint `env-N`, copy
   `name`, `sources`, `next_source`, `isolation` into the environment; set
   `project.environment_id`, `project.added_sources = []`,
   `project.project_key = <canonical home path>`, `project.isolation = null`
   (the override moved up; effective isolation is unchanged because the
   project inherits it back). The project keeps its `name`.
2. Parked projects are migrated the same way when unparked.
3. Write the config once, atomically, keeping the legacy project fields.
   Idempotent: a second load finds `environment_id` on every project and does
   nothing. Reversible: a bridge from before this change ignores
   `environments` and reads the legacy fields. (Store Migration Spec: renamed
   not deleted, idempotent, reversible.)
4. Tasks: **nothing moves**; `project_key` for a migrated project is the path
   every one of its rows already carries (1.6).
5. Secrets: nothing moves; `GrantScope::Project` rows keep working.
6. Workspaces: nothing moves; `.build-workspace.json` is read as today.
7. Runs, plans, scratch folders: nothing moves; their path value equals the
   migrated project's key (below).

Test: a fixture `config.json` with two projects, one parked, one with a
project-level isolation override, three tasks and one project-scoped secret
loads, migrates, serialises, reloads byte-identical, and every task number,
workspace directory, run and secret remains readable under the same ids.

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
today. Interaction recency (3.1) is derived, not stored (5.3).

**Path-keyed stores become key-keyed.** Several projects now share one home
path, so every store in 1.1 that used the canonical path uses
`project.project_key` instead. For migrated projects the value is identical,
so no row, folder or file moves:

- `tracker_tasks.project_key`: the column already holds exactly this value.
- `scratch_dir` (`conversation.rs:27-41`): derived from the key instead of the
  path; migrated projects keep today's folder, new projects get their own.
- `PersistedRun.project_path` / `PersistedPlan.project_path`: hold the key;
  recovery (`bridge/src/app/runtime/recovery.rs:333,401`) resolves key →
  project through the config. The field keeps its name until Stage 4.
- `task_project_maps`: key → project id.

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
(`projectView.js:160-180`, which skips to page 3 with that project fixed,
i.e. "new workspace in this project"), and Ctrl/Cmd-N.

**Page 1, "Where?"** Reads `environments`, `projects`, `workspaces` lists for
every device from the cache plus navigation visits
(`navigationVisits.js`, ≤20 at `uiAddress({view:"navigation", kind:"history",
sub:"visits"})`).

- Search field on top (same fuzzy rank as the switcher,
  `toolbarModel.js:93`).
- **Recent** section: targets interacted with in the last 24 h
  (`RECENT_AFTER_MS`, `inboxOrdering.js:3`), newest first, at most 5. A target
  is an environment or a diverged project. Its interaction time is
  `max(visit time of any project or workspace in it, last_activity_ms of any
  of its workspaces, created_at of any of its workspaces or projects)`, all
  already in the cache (`sessionSpans.js:16-24`). Empty: the section is
  omitted, not shown empty.
- **All** section: environments alphabetically, each a row with its identity
  badge (3.4), name, device tag when names clash, and "N projects · M
  workspaces"; diverged projects as indented rows "› Project (+K folders)"
  under their environment. Non-diverged projects are **not** listed here:
  picking an environment starts a new line of work, and existing lines of
  work are reached through the switcher (3.3). A project of an old bridge
  (no `environments` capability) is listed as itself.
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

**Page 3, "What are you working on?"**: title "New workspace in E" (or "in
E › P" for a template). One required **name** field ("Fix the inbox sort",
"Environments plan") that names the new line of work; the workspace takes the
same name and the branch preview `build/<slug>` (`branchNamePreview`,
`toolbarModel.js`) follows it. Isolation select with "Inherit E's setting", and
a read-only list of the directories the workspace will contain (effective
sources). "Create" calls `environment.create_project {environment_id, name,
from_project_id?, workspace}` through `deviceCall`, upserts the `projects` and
`workspaces` lists (`cacheCreatedWorkspace`, `createWork.js:47-52`, plus a
project upsert) and navigates to the workspace. Loading: the button shows
"Creating…" and the row appears in the inbox as *provisioning* from the
pending write (`writeOperations.js`). Error: the modal stays open with the
refusal under the button; the pending rows are removed. When opened with a
project fixed (project view "+", nav footer), the title is "New workspace in
P", the name is optional as today, and the call is `workspace.create`.

The word "project" does not appear on page 3; the user names the work. The
project exists afterwards as the crumb, the inbox block and the task board
under that name, and can be renamed in project settings (5.5).

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
  `N sources · M projects · K workspaces`, "Settings…". Under each, **every**
  project in it as an indented row: project badge, name, "+K folders" when
  diverged, `W workspaces · T open tasks`, "Settings…". "New environment…"
  opens wizard page 2 standalone.
- **Environment settings sheet** (new `sheets/environmentSettings.js`, modelled
  on `projectSettings.js`): General (name, editable → `environment.rename`;
  home path read-only), Sources (today's source cards,
  `sheets/projectSources.js`, against `environment.*_source`), Isolation,
  Secrets (environment-scoped list, Stage 2; the composer from #1006 with scope
  preselected), Projects in this environment (rows → project settings;
  "New project with extra folders…" opens a project sheet that is
  `project.create {environment_id, sources}`), Danger zone (delete: disabled
  with "Delete its N projects first" while refused).
- **Project settings sheet** (`projectSettings.js`, amended): General shows the
  name (now editable → `project.rename`) and "In environment E" with a link;
  Sources splits into "From E" (read-only cards with "Edit in E") and "Added
  here" (editable, `project.*_source`); Isolation says "Inherit E's setting";
  Danger zone: delete names the counts ("Deletes this project, its W finished
  workspaces and T tasks. Folders stay.") and is disabled while a workspace is
  unfinished (5.2).
- States: loading shows the cached list immediately and "loading…" only when
  the cache has nothing; error replaces only the still-empty list with the
  refusal text (`deviceProjects.js:87-89`); empty shows "No environments on
  this device yet" with the create button. Old bridge: the page lists its
  projects under a "Projects (this device needs an update to use environments)"
  heading with today's project settings.

### 3.3 Top nav

Today: `tb-switch` left chevron (#618) opens the global picker; the project
crumb; the local workspace selector (`toolbarRender.js:324-364`).

- **Switcher** (chevron, unchanged glyph and target) **lists projects**, as
  Zech asked, grouped under environment headings: Recent (projects interacted
  with in the last 24 h, newest first, at most 5), then each environment as a
  non-interactive heading with its badge and, beneath it, its projects sorted
  by recency with their badge, name and unread pill; a diverged project's row
  adds "+K folders". Each project row navigates to the project view (its
  task board and workspaces). Footer "New workspace…" opens the wizard.
  Heading "Switch to"; search placeholder "Search projects or workspaces".
  Workspaces keep appearing in search results as today
  (`navigationPickerModel.js:8-14`).
- **Crumb**: environment badge, then "E › P" with E in the muted colour and
  truncating first, as wide as its text (#523 rule). On ≤760 px only the badge
  and P show.
- **Workspace selector** (next to it, `tb-workspace`): clicking lists **every
  workspace of the project sorted by most recently interacted with**
  (`last_activity_ms` desc, then visit time, then created), the current one
  first and marked, finished ones under a "Finished" fold; footer "New
  workspace in this project" (wizard page 3 with the project fixed, a plain
  `workspace.create`) and "Manage workspaces". This replaces today's
  name/status order (`WORKSPACE_STATUS_ORDER`). Off a workspace route the
  selector reads "Workspaces ▾" as today.
- Paint: both menus render from `projects`, `environments`, `workspaces` and
  visits in the cache; unread badges from `navigationUnread`. The 900 px
  pinned-inbox geometry (#631) is untouched: the chevron and crumbs slide
  together; the badge adds no width beyond its 16 px glyph.
- Mobile: unchanged bottom-sheet behaviour (`shell.css:882-889`).

### 3.4 Badges for projects and workspaces

An identity badge, not a count. Deterministic from the key, so every device
and tab paints the same thing without storage. The **hue** belongs to the
environment: `hash(deviceKey(device, environmentId)) mod 12` from a 12-step
palette defined as new tokens `--badge-1…12` in both themes
(`styles.css:164-237`; nothing else in the client knows a colour,
`core/theme.js:1-14`). The **glyph** belongs to the row: the first two letters
of the project's name on a project badge, of the environment's name on an
environment badge. Shapes: environments a **filled** 16 px rounded square;
projects a **filled** 16 px circle in their environment's hue; workspaces an
**outlined** 16 px circle in the same hue. So siblings read as kin by colour
and as distinct by glyph, and a workspace is visibly "of" its project. Unread
counts keep today's `.badge` pill (`styles.css:471`) beside it. Both are
`aria-hidden`; the row text stays the accessible name.

Placement: switcher rows and wizard rows; the crumb; inbox project block heads
(`projectHeadHtml`, `inboxProjects.js:248-264`) in the grouped face; inbox
rows in the flat face where the project tag is today (`projectTagHtml`,
`inbox.js:964-967`: badge before the tag text); the workspace selector rows;
the environments settings list. The inbox status dot (`inboxStatusDot.js`) is
unchanged and sits where it does today.

### 3.5 Inbox

Grouping stays **by project** (`workspaceBlockFor`, `inboxProjects.js:78-102`),
which is now exactly the line-of-work grouping the user wants. The block head
reads "E › P" with the project badge; blocks of the same environment are
adjacent (sort by environment, then `byAnchor`), so one environment's lines of
work sit together. The "New project" button (`inboxShell.js:358-363`) becomes
"New workspace" and opens the wizard. Row "+" on a block head
(`data-project-create`) opens wizard page 3 with that project fixed.

---

## 4. Stages

Each slice is one branch from `dev`, independently mergeable, TDD, with its
own fixture/version updates where it touches the wire. "Bridge" slices run
`cargo test`/clippy/fmt; "SPA" slices run lint/test/build; both run gitleaks
and semgrep per AGENTS.md.

### Stage 1: MVP, fully functional end to end

Goal: a user on a fresh or existing device can start a new line of work in
an existing or new environment through the wizard and land in its workspace
with an empty task board; a second workspace in that line of work comes from
the nav; the model is migrated; nothing an old client does breaks.

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| 1.1 | **Bridge: environment model + config migration** (`app/environments/`, `Project.environment_id/project_key/added_sources`, `config_value_with_project`, load; `tracker_project_path` and the path-keyed stores read `project_key`) | every existing project loads as one project under its own environment; config round-trips; second load is a no-op; a pre-change bridge still loads the written file; every task, run and scratch folder of a migrated project resolves as before | `config_migration_tests.rs`: fixture with 2 projects (one parked, one isolation override) → environments; idempotence; legacy reload; `next_source` continuity; `sources_for` = env ++ added; `migrated_project_key_is_home_path`; `tasks_readable_after_migration` |
| 1.2 | **Bridge: wire 3.37.0** (`environment.list/create/create_project/*_source`, `project.rename`, `project.list` fields, `board.list.environments`, capabilities, board-scope push, fixtures, manifest) | all `api_contract.rs` tests pass; `project.create` without `environment_id` makes an implicit environment; `environment.create_project` returns a project with a fresh key and its first workspace; two projects created in one environment number tasks independently from #1 | contract fixtures for the new verbs; `implicit_environment_from_legacy_project_create`; `create_project_mints_fresh_key_and_workspace`; `sibling_projects_number_tasks_independently`; push test that an `environment.create` lands as a board change |
| 1.3 | **Bridge: diverged projects** (`project.create {environment_id, sources}`, `project.*_source` on added sources, `diverged` flag, refusal of editing env sources via project verbs with the old-client routing of 5.9, template copy via `from_project_id`, exact-duplicate relaxation of overlap within an environment, key-derived scratch folder) | adding a folder to a project flips `diverged`; env-source ids refused through `project.remove_source`; a project created from a template has the same added paths under new ids; the same folder as an added source in two environments is still refused; a new project's coordinator and runs recover to that project after a restart | `added_source_makes_project_diverged`; `project_verbs_refuse_environment_source_ids`; `template_copies_added_sources_with_new_ids`; `added_source_duplicate_allowed_within_environment_only`; `new_project_runs_recover_to_their_project`; `migrated_project_keeps_legacy_scratch_dir` |
| 1.4 | **SPA: capability + cache plumbing** (`environment-support` record, `capabilitiesOf().environments`, `NO_CAPABILITIES`, `environments` session list, `writePushedBoard` third list, `readSyncLists` asks `environment.list` when supported) | a greeting with the capability writes the record; a board push updates the cached environments list; without the capability nothing new is asked | `environmentSupportWiring.test.js`, `cacheSyncChanges.test.js` additions, `namedCapabilitiesWiring.test.js` |
| 1.5 | **SPA: wizard pages 1 and 3** (`core/createWorkspaceWizard.js`, target model `environmentTargets()` with recents, replaces `createWork.js` modal; every caller switched; fixed-project mode for "new workspace in this project") | opens from switcher, inbox, project view, Ctrl/Cmd-N; recents last 24 h on top; diverged projects nested, plain projects not listed; old bridge lists projects and creates a workspace in one; create lands on the new workspace with the new project in the cache; fixed-project mode calls `workspace.create`; offline device shows the pending row then the error | `createWorkspaceWizardModel.test.js` (recents, nesting, fallback, fixed mode), `createWorkspaceWizardDom.test.js` (pages, keyboard, error, pending rows), `createWork.test.js` retargeted |
| 1.6 | **SPA: wizard page 2** (`newRepo.js` reshaped into `environment.create`, draft address renamed, Back keeps the draft) | a folder or URL creates an environment and the wizard continues to page 3 with it selected; old bridge falls back to `project.create` | `newProjectDom.test.js` → `newEnvironmentDom.test.js`; draft persistence test |
| 1.7 | **SPA: top nav** (switcher lists projects grouped by environment with recents, "E › P" crumb, workspace selector by recency, "New workspace in this project") | project rows grouped and sorted by recency; workspace rows sorted by `last_activity_ms` desc then visit; current first; finished folded; #523/#618/#631 geometry tests still pass | `toolbarModel.test.js` (new `switcherModel` grouping, `workspaceMenuModel` order), `toolbarDom.test.js`, `navigationPicker.test.js`, `test/browser/pinnedInboxToolbarLayout.test.js` unchanged |
| 1.8 | **SPA: settings page MVP** (`#/account/environments`, list with every project nested, environment settings sheet with General + Sources + Projects, project sheet amendments incl. editable name) | every existing project appears under its environment; sources edit via `environment.*_source`; project sheet shows inherited vs added and renames; old-bridge notice | `environmentsSettingsDom.test.js`, `projectSettings.test.js` amendments, router test for the new route |
| 1.9 | **Docs**: `ARCHITECTURE.md` (Workspaces and worktrees 1636, RPC 163, Capability gating 5000), `docs/using.md`, Tasks Spec note that `project_key` is the project's key (path for migrated projects), this plan's status | sections describe the code as merged | n/a (docs) |

Done when: on dev, a user starts a new line of work in a brand-new environment
from the inbox in three clicks and its task board is empty while the sibling
project's board is untouched, an old SPA tab still lists and creates projects,
and `project.list` for a migrated device shows every project with
`environment_id` and its old tasks.

### Stage 2: full feature

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| 2.1 | **Bridge: environment secrets** (`GrantScope::Environment`, store schema 26 table rebuild, membership check, revoke on environment delete, name override resolution, `secrets.*` scope param, `secrets.changed` scope, `secrets.environmentScope` capability, 3.38.0) | an environment secret is readable by agents of every project in it and by none outside; project secret of the same name wins; a sibling project never reads another project's secret; deletion revokes | `environment_scoped_secret_readable_across_projects`; `project_secret_overrides_environment_name`; `sibling_project_cannot_read_project_scoped_secret` (one per mechanism); `environment_delete_revokes_environment_scope` (twin of `project_delete_entry_point_revokes_project_scoped_secret`); leak sweep row 14 rerun |
| 2.2 | **Bridge: environment rename/delete/isolation, project delete with tasks** (`environment.rename/delete/set_isolation`, `project.delete {tasks}`, refusals, parked-project unpark migration) | rename changes display only; environment delete refused with "Delete its N projects first"; project delete refused while a workspace is unfinished, otherwise removes the project's tasks, reviews and delivery intents under its key; isolation resolves workspace → project → environment → device | `environment_rename_keeps_ids_and_paths`; `environment_delete_refused_while_projects_exist`; `project_delete_removes_its_tasks_only`; `isolation_resolution_order` |
| 2.3 | **Bridge: MCP** (`add_environment_source`/`remove_environment_source`, reworded `add_project_source`, `create_workspace`/`list_tasks`/`create_task` unchanged, templates) | project agent can add a folder to its environment; descriptions mention environments; a Coding-surface agent still has no such tool | `mcp_project_surface_lists_environment_source_tools`; template render snapshot |
| 2.4 | **SPA: secrets in environment settings + composer scope** (#1006 composer gains "This environment" scope; chips show "overrides environment") | create/list/revoke env secrets from the sheet; old bridge hides the scope | `secretsWire.test.js`, `environmentSettingsSecrets.test.js` |
| 2.5 | **SPA: environment rename/delete/isolation and project delete in settings**; project sheet "New project with extra folders…" | refusal reasons shown verbatim and the button disabled; project delete confirm names the task and workspace counts; rename repaints every crumb and block head from the cache | `environmentsSettingsDom.test.js` additions, `projectSettings.test.js` |
| 2.6 | **SPA: inbox** (block heads "E › P", adjacency sort, "New workspace", block "+") | grouped face shows environment and project names with badges; sort stable | `inboxProjects.test.js`, `inboxDom.test.js` |
| 2.7 | **SPA: badges** (tokens, `identityBadgeHtml`, placements of 3.4) | identical hue across tabs for one environment; sibling projects share the hue and differ by glyph; both themes pass contrast on the badge glyph | `identityBadge.test.js`, theme token test |
| 2.8 | **SPA: wizard "Add a folder to this work only"** on page 3, creating the new project diverged in the same `environment.create_project` call (added `sources` param) | creating with an extra folder yields `diverged: true` in one flow; the project then appears as a template row in the wizard | `createWorkspaceWizardModel.test.js` (diverge path), `createWorkspaceWizardDom.test.js`; contract fixture for the new param |

### Stage 3: polish

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| 3.1 | Wizard recents across devices with device tags; keyboard-only path; mobile sheet motion; reduced-motion | a11y audit of the dialog passes; ≤760 px layout screenshot test | browser layout test |
| 3.2 | Workspace selector: "Finished" fold, counts, empty copy ("No workspaces in P yet") | | `toolbarDom.test.js` |
| 3.3 | Environment view (`#/environment/<id>`: its projects as cards with open-task and workspace counts, its sources); project view gains an "Environment" link | | router + view tests |
| 3.4 | Discoverability: first-run hint on the wizard ("Each thing you work on gets its own task board and workspaces; they all share the environment's folders"), empty-state copy on the inbox and on a new project's empty task board | copy approved by Zech | DOM tests |
| 3.5 | `docs/using.md` and `ARCHITECTURE.md` terminology pass; Terminology Audit amended with retained contracts (RPC names unchanged, routes unchanged) | | n/a |

### Stage 4: cleanup of old systems

Safe once Stage 2 has been live on `main` for one release and the SPA's
`SPA_API_RANGE` floor (`spa/src/core/bridgeApi/index.js`) is raised to
`>=3.38.0`.

| # | what goes | when safe | test |
| --- | --- | --- | --- |
| 4.1 | Legacy project fields in `config.json` (`path`, `base_branch`, `next_source`, `sources` on project entries); `Project.repo_path/base_branch/next_source` fields replaced by accessors on the environment | after one release with rollback no longer needed (Zech's call); loader keeps reading them for one more release, writer stops | migration test drops the legacy fields and reloads |
| 4.2 | `createWork.js` modal and `CREATE_KINDS`; `newRepo.js` project path; "Add project" buttons | once every caller uses the wizard (end of Stage 1) | remove tests |
| 4.3 | `project.create` without `environment_id` (implicit environment) → refused with "name an environment" | when `SPA_API_RANGE` floor ≥ 3.37.0 and the desktop app ships that SPA | contract fixture flips to the refusal |
| 4.4 | `WORKSPACE_STATUS_ORDER` name/status sort in `toolbarModel.js` | end of Stage 1 | |
| 4.5 | `PersistedRun.project_path`/`PersistedPlan.project_path` renamed `project_key`; `schema.rs:226` comment reworded from "canonical home path" to "project key" | Stage 2 merge (rename only; values unchanged) | entity round-trip test |
| 4.6 | The old-client routing of `project.update_source` to environment sources (5.9) | when `SPA_API_RANGE` floor ≥ 3.37.0 | contract fixture flips to the refusal |

---

## 5. Oversights and open questions

Each with a recommended answer. Items marked **(Zech)** are still open and
need his call before the slice that depends on them starts; the rest are
recorded decisions he can overturn.

1. **Picking a diverged project in the wizard.** Zech lists it "as its own
   environment", so picking it must behave like picking an environment: a
   **new** project (new task board) whose added folders are copied from it,
   with the diverged project acting as a template (1.2). The alternative,
   "a new workspace in that project", would put new work on an old task
   board, which is what this change exists to stop. *Recommend* template
   semantics; the overlap rule is relaxed only for exact duplicates inside one
   environment. **(Zech)**: confirm before 1.3.
2. **Project delete and its tasks.** A project is one line of work, and after
   this change a new project's key is never reused, so "re-add the folder and
   find the tasks" no longer applies to it. *Recommend*: `project.delete` is
   refused while a workspace is unfinished and otherwise removes the project's
   tasks, reviews and delivery intents with it; the confirm names the counts.
   Migrated projects keep the path key, so re-adding the same folder as a new
   environment and project would find the old board; *recommend* accepting
   that quirk rather than migrating rows. Environment delete is refused while
   any project remains, so there is nothing further to decide there.
   **(Zech)**: confirm tasks go with the project.
3. **What "recent" counts as interaction.** *Recommend*: `max(visit to a
   project or workspace in the target, workspace last_activity_ms, workspace
   or project created)` within 24 h, computed in the SPA from cache; no new
   bridge storage. Agent-only activity (a worker posting at 3 am) counts
   because `last_activity_ms` is conversation activity
   (`workspaces/mod.rs:433`), which is what you want to see first in the
   morning. Visits are per tab/device profile, so recents differ slightly per
   device; acceptable.
4. **Environment rename.** *Recommend*: display-only; never moves folders;
   project names do not follow; ids, task keys, workspace roots unchanged.
5. **Project names, and the word "project".** The wizard asks "What are you
   working on?" and that one answer names the project and its first workspace
   (3.1). *Recommend* a required name (a project titled after a branch slug
   would make the inbox and task board unreadable), editable later through
   `project.rename`; "project" stays a user-facing word for the line of work
   in the crumb, the inbox head, settings and the task board, while the wizard
   never says it. **(Zech)**: confirm the required name and the vocabulary
   before 1.5/1.7 copy; record in the Terminology Audit (#537).
6. **Badges: identity or counts?** The ask says "badges for projects and
   workspaces". *Recommend* identity badges (3.4), environment hue with a
   project glyph, beside the existing unread pills. **(Zech)**
7. **"Environment" already means dev/prod in secrets.** `SecretEnvironment
   {Dev, Prod}` (`bridge/src/thread/secrets.rs:14-33`) is what `buildctl --env`
   and the secret runtime CLI mean by the word. Two meanings of "environment"
   in one codebase, one of them in a security-sensitive CLI, is a trap.
   *Recommend*: rename the secrets channel to **`stage`** (`SecretScope.stage`
   already exists, `api/v1/secrets.rs:21-25`) in a mechanical slice before
   1.1, or pick another user-facing word for this feature (**"space"**,
   **"codebase"**). **(Zech)**: the plan uses "environment" throughout until
   told otherwise.
8. **Secrets security.** Checklist rows 8 and 11 generalise cleanly; the new
   risk is **widening**: a project-scoped secret must not become readable by a
   sibling project by any path (attachment resolution, CLI release,
   `attached_secrets.md` rendering), and sibling projects are now the common
   case. *Recommend*: a negative test per mechanism (release authority, list,
   attach, audit), the override rule being project-wins only, and no copying
   of project secrets into a project created from a template. Moving an
   existing project-scoped secret "up" to the environment is **not offered**
   (re-create instead) so no value is ever re-sealed by the bridge.
9. **`project.create` and `project.*_source` compatibility.** An old SPA's
   "Add project" makes an implicit environment, and its project settings then
   shows environment sources as editable through `project.update_source`.
   *Recommend*: accept `update_source`/`sync_source` on an environment source
   id from a project with no added sources and route it to the environment;
   refuse `remove_source` and any call from a diverged project with "edit it
   in E". Removed in 4.6.
10. **Sources added to project vs environment.** *Recommend*: the UI defaults
    everything to the environment; "add to this project only" is a secondary
    action in project settings, wizard page 3 (2.8) and the MCP
    `add_project_source`, whose description says it makes the project diverge
    so a project agent does not fork its project by accident.
11. **Multi-device and E2EE.** Environments are per device, like projects; no
    cross-device identity. Same name on two devices is two environments with
    device tags. Nothing new crosses the relay in plaintext: environment names
    and paths travel inside the same sealed channel as project rows today.
    Open: should the account later sync "environment of the same remote URL"
    across devices? *Recommend*: out of scope; note in the Scope doc.
12. **MCP tools.** `create_workspace`, `list_tasks` and `create_task` are
    unchanged: an agent works in its project and on its project's board. Add
    `add_environment_source`/`remove_environment_source` on the Project
    surface. *Recommend* no `create_project` tool: starting a new line of work
    is the user's act. `list_projects` (router) groups projects by environment.
13. **Inbox grouping.** Blocks per project are now the right grouping by
    definition (one block per line of work). *Recommend* "E › P" heads and
    adjacency by environment; no block per environment.
14. **Mobile.** The wizard is a bottom sheet; pages slide; the workspace
    selector's recency list is the same sheet as today's menu. Open: page 2's
    folder browser on a phone is already awkward (`sheets/browser.js`);
    *recommend* the Git URL path is first on mobile.
15. **Compaction policy and project agents** stay per project (one coordinator
    per line of work, which is what the project agent already is); the
    environment has none.
16. **Several projects on one path.** Runs, plans, the coordinator scratch
    folder and the task map were keyed by path. The plan keys them by
    `project_key` (1.6, 2.4), which equals the path for migrated projects, so
    nothing moves and new projects get their own. Recorded because it is easy
    to miss and would otherwise bind a new project's runs to its sibling.
17. **No merging of existing projects.** Migration yields one environment per
    existing project. Two existing projects that are really the same codebase
    (impossible today because of overlap refusal, so only after a user
    re-registers) stay separate; "move project to another environment" is not
    planned. *Recommend* out of scope; a user starts new work in the right
    environment and lets the old project finish.
18. **Secrets schema rebuild.** The grant scope CHECK constraint makes the
    environment scope a table copy, not an `ALTER`. *Recommend* it stays in
    Stage 2 (2.1) with the leak sweep rerun, and that Stage 1 ships no secrets
    change at all.
19. **Wire naming.** `environment.create_project` as one verb that also cuts
    the first workspace, versus extending `project.create` and calling
    `workspace.create` after it. *Recommend* the new verb: one audited
    operation, no half-made project if the workspace fails, and the existing
    fixtures and `params.strict` shapes stay untouched.

---

## 6. Risks

- **Getting the model wrong again.** The first draft scoped tasks to the
  environment; Zech's correction is now the opening section. Mitigation: 1.2
  carries `sibling_projects_number_tasks_independently` as a named acceptance
  test, and the Done-when line for Stage 1 checks it by hand.
- **Scope creep into secrets.** A 100/100 checklist today. Mitigation: Stage 1
  touches no secret; Stage 2.1 carries its own checklist rows and negative
  controls per mechanism, including the sibling-project case.
- **Config migration on a live device.** A bug here parks or drops projects.
  Mitigation: migrate before serving, keep legacy fields, byte-identical
  reload test, parked never dropped, rollback path proven in the test.
- **Key confusion.** Migrated projects carry a path as key, new ones a uuid.
  Mitigation: one accessor (`project.project_key`) and no code that parses the
  key; a test that a path-shaped key and a uuid key resolve the same way.
- **Two id namespaces per project** (environment sources and added sources)
  confusing readers. Mitigation: one counter on the environment; added
  sources are plain `source-N` from it.
- **Old SPA on new bridge** silently creating environments the user did not
  name. Mitigation: implicit environment takes the project's name; the next
  settings visit on a new SPA shows it; nothing is lost.
- **Many small projects.** Every wizard run makes a project; the inbox and
  switcher get one block per line of work. That is the intent, but finished
  work must fold away: the "Finished" fold (3.2) and project delete (2.2)
  carry it.
- **Recents differing per device/tab** (visits are local). Accepted; stated in
  5.3.
- **Nav geometry regressions** (#523/#618/#631 have browser layout tests).
  Mitigation: badges are fixed 16 px; the existing layout tests stay green as
  acceptance for 1.7.
- **Word collision with the secrets dev/prod channel** (5.7). Mitigation:
  decide the word before any copy or verb name lands.

---

## 7. Security checklist delta

New rows for `Project Sources Security Checklist.md` (Stage 1) and
`Secrets Security Checklist.md` (Stage 2), each with a named test:

| checklist | control | test |
| --- | --- | --- |
| Project Sources | Overlap refusal spans every environment and every project's added sources on the device; the only relaxation is an exactly equal path as an added source of two projects in the **same** environment; containment, remotes and cross-environment duplicates stay refused | `source_overlap_refused_across_environments`; `added_source_duplicate_allowed_within_environment_only` |
| Project Sources | `project.*_source` verbs cannot edit or remove an environment source through a diverged project; the refusal names the environment | `project_verbs_refuse_environment_source_ids` |
| Project Sources | Config migration writes nothing until every project is assigned; a failure leaves the old file intact (atomic temp + rename, `persistence.rs:86-104`) | `migration_failure_leaves_config_untouched` |
| Project Sources | Environment and project rename are bounded one-line text with no control characters and never a path | `rename_is_one_bounded_line` |
| Secrets (8) | Environment scope derives from the issued session's project's environment at read time, never from a selector | `environment_membership_revalidated_per_read` |
| Secrets (11) | Environment deletion revokes environment-scoped secrets before the registration is removed; project deletion revokes that project's scope and leaves environment scope intact | `environment_delete_revokes_environment_scope` |
| Secrets (new) | A project-scoped secret is never released to a sibling project's agent by list, attach, CLI release or audit, and is never copied by `environment.create_project` | one negative test per mechanism |
| Secrets (19) | `secrets.environmentScope` capability, fixtures, manifest and version pins; an older client never sees the scope | `secrets_contract_includes_environment_scope` |
| Tasks | Two projects in one environment number and list tasks independently; a task link to a sibling project's workspace is refused as today | `sibling_projects_number_tasks_independently`; `link_workspace_of_sibling_project_refused` |
| Tasks | `project.delete` removes only rows under the deleted project's key | `project_delete_removes_its_tasks_only` |
| Navigation (#523) | Badges add no interactive element and no accessible name; layout tests at 900 px unchanged | `pinnedInboxToolbarLayout.test.js` |

Rows touched but unchanged in meaning: Secrets 13–16 (values never in SPA
persistence or history; the environment sheet renders metadata only).
