# Environments: a layer above projects

Original planning baseline for **#1028**, 2026-11-07, before implementation:
code inspected at `f1376d41` (`origin/dev`), wire `3.36.0`, store schema 25.
The merged status below and checkpoint amendments record subsequent delivery.

Companions: `Project Workspace Spec.md` (#522), `Navigation Spec.md` (#523,
#618, #631), `Project Workspace Terminology Audit.md` (#537), `workspaces.md`,
`Store Migration Spec.md`, `Project Sources Security Checklist.md`,
`Secrets Security Checklist.md`, `Tasks Spec.md` (scope rules, lines 1567–1582),
`ARCHITECTURE.md` "Durable write foundation (#739)" (line 1269).

## The model in three lines

**Merged status at `750e70576648b05c73e5cff3ae162e82468a2518`:** M0–M3.3
are on dev. Wire 3.39.0 advertises `projects.archive`, `projects.rename` and
`environments.rename`, alongside the M1/M2 capabilities. The wizard supports
new environments and project-only extra folders; settings supports archive,
restore, rename and environment source/isolation edits. Stage 2 features remain
unavailable as listed under M3. The seven Stage 1 manual dev journeys remain
pending; implementation completion does not claim their acceptance.

- **Environment** = the source directories and the secrets. It is where the
  code lives. It has no tasks, no agents and no workspaces of its own.
- **Project** = **one line of work** inside an environment, with **its own
  task board**, its own project agent and workers, and optionally folders of
  its own. Many projects share one environment; most add nothing to it and
  differ only in what they are working on. Separating their tasks is the
  reason for this change.
- **Workspace** = unchanged. A checkout of its project's effective sources
  (environment sources plus the project's added folders), cut as today.

The user creates a **workspace** through one wizard. Its first page lists
**existing projects first, then environments**. Picking a project adds a
workspace to that line of work and its board. Picking an environment starts a
**new project** in it (a fresh board) and its first workspace; the project is
implicit. "New environment…" adds a page before the naming page. The wizard
collects drafts only; nothing exists until the final **Create**, which then
shows its progress step by step. Two projects may list the same folder.

## 0. What this revision settles, and what it does not

Everything below is integrated through the model, wire, UX, stages, tests,
risks and checklist, not appended.

**Settled by Zech on #1028** (requirements, not up for re-litigation here):

- Tasks belong to **projects**, never environments; that is the point of the
  change (tc-01M6VFR6X191KBKVPC6YH3C7P6).
- The wizard lists **existing projects first, then environments**, so it is
  also how a workspace is added to an existing project
  (tc-01M6VNNWNK8R28YMHC9SYFYH3Q).
- Workspace-first creation through a wizard; "new environment" as a page of
  the same flow; the project is implicit; a settings page for environments
  and projects; the two-part top nav; badges in nav and inbox (task body).
- Projects can be **archived** ("Yes we should have archiving"); only the
  archive policy for live work is ours to recommend (5B.1).

- Zech's answers in the review workshop, recorded in Environment UX's brief
  (tc-01M6VX9S2K8FWS9KZ0E149XRCG): the wizard collects **drafts only** until
  the final Create, with a real progress screen and honest partial and
  unknown outcomes; projects **may share source directories**; extra project
  folders are an **Advanced** option of the one creation flow, with no
  separate project-create sheet; **device-name labels** on projects and
  environments wherever they are listed; one **count-text rule**
  (`N open tasks` when nonzero, otherwise `T tasks`, including `0 tasks`);
  and no "re-adding a migrated folder recovers its old board" claim.

**Implementation policy chosen here** (ours; Zech can overturn): archive
lands in M3, before sustained dogfooding, as sequencing; kind glyphs stand in
for identity badges until Stage 2; the creation contract of 2.3; the identity
split of 1.3; shared-directory semantics of 1.4.

**Still ours to recommend, Zech's to decide** (5B): the archive policy for
projects with live agents or attention; identity glyph design; the word
"environment" versus the secrets dev/prod sense; a required project name;
whether a project's tasks are deleted with it.

---

## 1. Model

### 1.1 Before M0 (historical baseline)

- A **project** (`bridge/src/app/projects/mod.rs:135`) is `{id: proj-N, name,
  repo_path, base_branch, is_git, sources: Vec<ProjectSource>, next_source,
  orch, isolation: Option<Isolation>}`. The first source is the project's
  home; `name` is the home folder's name and is read-only in settings
  (`spa/src/sheets/projectSettings.js:22-25`). The id **is persisted**
  (`"id": p.id`, `bridge/src/app/config/persistence.rs:49`) and restored
  (`project_candidate_at(…, restored_number)`, `projects/mod.rs:426`). Source
  ids `source-N` are never reissued (`next_source`, `persistence.rs:53`).
- A **workspace** (`bridge/src/workspace.rs:84`) is `{id, project_id, name,
  root, status, directories, isolation, managed, created_by_agent, locked}`;
  each directory is cut from one `WorkspaceSource {id, name, mount, path,
  is_git, base_branch}` (`workspace.rs:110`). New workspaces take the
  project's sources at that moment (`AppState::sources_for`,
  `projects/mod.rs:566`); later edits are forward-looking. The manifest
  `.build-workspace.json` (`workspace.rs:37`) records `project_id` and each
  directory's `source_id`, so one source folder already backs any number of
  isolated checkouts (worktree or rift, `bridge/src/isolation/`).
- **Provisioning** already has the shape the wizard needs.
  `WorkspaceCreateWork` (`bridge/src/app/workspaces/mod.rs:36`) runs as
  deferred Git work off the app mutex: base sync before the cut, then either
  `registry.provision` (published, `fail_if_still_provisioning` on
  invalidate) or `registry.provision_unpublished` with a **rollback** of the
  directories it made and removal of the incomplete root (`mod.rs:90-123`,
  `rollback` at `:257`); per-source work is `materialize_source` (`:162`),
  shared with "add a directory to a live workspace". A failed preparation
  removes only a root this create privately made (`preparation_failed`,
  `:217`). Deferred creation can `recover_existing` a saved reservation.
- **Durable writes.** Every request frame may carry typed `write` metadata
  `{operation_id, after?}` (`ARCHITECTURE.md:192`); the SPA mints or forwards
  one per write (`submitWrite`, `spa/src/core/bridgeApi/v1/writeMethods.js:20`)
  and persists intent and outcome in `writeOperations.js` /
  `writeOperationCache.js`. Schema 18 holds `mutation_commands`, owner order,
  reservations and a terminal outbox; `writes.status` and `writes.subscribe`
  (`bridge/src/api/v1/writes.rs:10-12`, `app/mutations.rs`) read them. **No
  verb executor is enabled yet** ("semantic method enablement belongs to Step
  6b", `ARCHITECTURE.md:1308`). Workspace creation today is *not* journaled
  end to end.
- **Git serialisation.** `source_sync::SyncLock` (`bridge/src/source_sync.rs:412`)
  is keyed by `sync_lock_identity`: the canonical **common Git directory**
  when the path is a repository, else the canonical path (`:473-478`); linked
  worktrees share one lock. **Worker holds** are per project:
  `refuse_moving_sources_under_workers(project_id)`
  (`bridge/src/app/projects/source_holds.rs:70`), called from
  `source_update.rs:78` and `requests.rs:499`.
- **Secrets** are scoped `GrantScope::Agent {…} | Project {project_id}`
  (`bridge/src/secrets/model.rs:13`); project membership is revalidated per
  read and project scope revoked on project deletion (Secrets checklist rows
  8 and 11).
- **Tasks** belong to exactly one project (Tasks Spec 1567–1582), keyed by
  the project's canonical home **path**: `tracker_tasks.project_key`,
  `UNIQUE (project_key, number)` (`bridge/src/store/schema.rs:226-245`), via
  `tracker_project_path(project_id)` (`bridge/src/app/tracker/mod.rs:716`),
  which 20 call sites use (`tracker/*.rs`, `projects/deletion.rs:238`,
  `workspaces/reclaim.rs:788`). This works today only because overlap
  refusal (`projects/requests.rs:285-365`) guarantees no two projects share a
  path.
- **Other path-keyed identity.** Persisted runs and plans carry
  `project_path` (`bridge/src/store/entities.rs:46,107,274`); recovery
  re-resolves it with `add_project(repo_path)`, which **finds a project by
  canonical path** (`projects/mod.rs:407-411`;
  `runtime/recovery.rs:329-332,396-399`). The coordinator's scratch folder is
  `scratch_dir(state_root, project.repo_path)` (`projects/conversation.rs:36`,
  callers `conversation.rs:101`, `conversations/senders.rs:96`, recovery
  `:468-471`). `task_project_maps` maps `repo_path` ↔ id
  (`tracker/read.rs:39-50`). Review verdict delivery compares
  `project.repo_path == delivery.project_path`
  (`tracker/reviews/verdict_delivery.rs:155,256`).
  `ProjectRegistry::bind_retained_entities` binds unbound entities whose
  retained path canonicalises to the project's repo (`project_registry.rs:136`).
- Every agent MCP tool takes its project from the caller's binding
  (`ProjectRegistry::project_id_of`, `project_registry.rs:344`;
  `bridge/src/mcp.rs:1521-1526, 327-335`).
- Projects and ids are **per device**; the SPA keys by
  `deviceKey(deviceId, projectId)` (`spa/src/core/deviceKey.js:11`); the inbox
  shows a device name on a project only when names clash (`deviceTagOf`,
  `spa/src/core/inboxProjects.js:178-186`).

### 1.2 Environment model (M0/M1 merged; later extensions planned)

```
device
 └─ environment  env-N          owns: name, sources[] (references), next_source,
    │                                  secrets (env scope, Stage 2), default isolation
    ├─ project    proj-A        owns: environment_id, project_key, home_path, tasks (own numbering),
    │   │                              added_sources[] (references), isolation override, compaction,
    │   │                              project agent + workers, archived_at
    │   ├─ workspace ws-…       owns: project_id, directories (one per effective source),
    │   │                              isolation, name, lock          (unchanged)
    │   └─ workspace ws-…
    ├─ project    proj-B        another line of work on the same code, its own board
    └─ project    proj-C        diverged: added_sources non-empty ("+K folders")
```

**Environment** (new, `bridge/src/app/environments/`): `{id: env-N, name,
sources: Vec<ProjectSource>, next_source, isolation: Option<Isolation>}`.
Ids are persisted like project ids. It owns the source **references**, the
one `source-N` counter, and (Stage 2) the scope of environment secrets. Its
first source is its **home**.

**Project**: keeps `id`, `name`, `orch`, `isolation`, compaction, agents, and
gains `environment_id`, `project_key` (1.3), `home_path` (1.3),
`added_sources: Vec<ProjectSource>` and `archived_at: Option<ms>` (1.9).
**Effective sources** = `environment.sources ++ project.added_sources`, in
that order; that is what `sources_for` returns and what a workspace cut
contains. Added source ids come from the **environment's** counter so a
workspace's `directories[].source_id` is unique within the environment and
never reissued. `name` becomes editable (M3); it titles the crumb, the inbox
block and the board. `repo_path`/`base_branch` stay as fields derived from
`home_path` until Stage 4 removes them.

**Many projects per environment is the normal case.** Nothing distinguishes
two projects with no added folders except their tasks, agents and workspaces.
There is no base or canonical project. "Create in E" always makes a **new**
project (2.2 `environment.create_project`); a second workspace in a line of
work comes from picking that project (page 1) or "New workspace in this
project" (3.4), both today's `workspace.create`.

**Diverged project.** A project diverges from its environment **iff
`added_sources` is non-empty**. Isolation or compaction overrides, an agent
roster or project-scoped secrets do not count: none changes what a workspace
cut from it contains, and a new project never inherits any of them. The
bridge computes `diverged = !added_sources.is_empty()` and sends it on every
project row; the SPA never derives it. A diverged project is listed in the
Projects section as itself with "+K folders"; it is not a template.

**Workspaces are unchanged.** A workspace still names its `project_id` and
snapshots effective sources at the cut. `environment_id` is derivable and is
added to `workspace.list` rows for grouping; the manifest gains the field on
next write and does not require it to load.

### 1.3 Identity versus location

The home path stops identifying a project the moment two projects share it,
so every consumer that today reads `repo_path` is classified **per usage**
as needing *identity* or *location*:

- **`project_key`** (identity): a stable string persisted in `config.json`.
  Migration sets it to the canonical home path each existing project already
  uses, so **not one `tracker_tasks` row changes**. New projects get
  `project:<uuid>` at creation, never derived from `proj-N`. Nothing parses
  a key; a path-shaped key and a uuid key are opaque strings that resolve
  through one accessor, `AppState::project_by_key`.
- **`home_path`** (location): a real directory, the environment's home for
  a new project, the old `repo_path` for a migrated one. Used wherever a
  process must stand somewhere or a repository must be opened.

| usage | today reads | needs | change |
| --- | --- | --- | --- |
| `tracker_project_path` and its 20 callers (`tracker/mod.rs:716`) | canonical `repo_path` | identity | returns `project_key`; callers unchanged, column name kept |
| `task_project_maps` (`tracker/read.rs:39-50`) | `repo_path` ↔ id | identity | `project_key` ↔ id, one-to-one by construction |
| `tracker_task(task_id)` (`tracker/mod.rs:708`) `find_by_canonical_path(task.project_path)` | path → project id | identity | `project_by_key(task.project_path)`; without this a uuid-keyed task cannot be looked up at all |
| review revision-dispatch recovery (`tracker/reviews/revision_dispatch.rs:97`) | path → project id, else the notice is silently dropped | identity | `project_by_key`; a failed-review notice for a uuid-keyed project must not vanish |
| session summary recovery (`session_summaries.rs:113`) pool key → project id | path → id | identity | `project_by_key`; pool keys for new projects are their uuid keys |
| `resolve_task_attachment` (`tracker/attachments.rs:121-125`) | `local_attachments_dir()`, the bridge's attachment store | **neither** | unchanged: containment stays inside the attachment store and is never widened to a project's home or source files |
| `scratch_dir(state_root, project.repo_path)` (`conversation.rs:36,101`; `senders.rs:96`) | path digest | identity | digest of **`project_key`** (`scratch_key(key)`); a migrated project's key equals its path, so today's folder is kept byte for byte; siblings never share scratch |
| `is_project_scratch(state_root, record.project_path, root)` (`recovery.rs:468`) | record path | identity | record carries the key (below); same digest |
| `PersistedRun/PersistedPlan.project_path` (`entities.rs:46,107`; `verdict_delivery.rs:70`) | path string | identity | field holds the key; renamed `project_key` in Stage 4 only. Legacy rows hold the migrated project's path, which **is** its key: they match exactly one project and are never ambiguous |
| `recovery.rs:329-332, 396-399` `add_project(PathBuf::from(record.project_path))` | path → find-or-add project | identity, then location | resolve `project_by_key(record.project_path)`; when found, take its `home_path` for `exists()`, worktree and repo work. A key that resolves to no project (deleted) keeps today's "repo missing" branches with the key in the message; recovery **never** adds a project from a record |
| `recovery.rs:330-331, 397-398, 552-588` `repo_path.exists()`, "project repo missing at" | filesystem | location | `home_path` of the resolved project |
| `ProjectRegistry::retain_entity_path` / `project_path_for` (`project_registry.rs:458,350`) | retained path text | identity (display fallback) | retains the key; display shows the resolved project's `home_path` first, then the retained text, as today |
| `bind_retained_entities(project_id, repo)` (`project_registry.rs:136`) | canonicalise retained path == repo | identity | bind when retained key == that project's `project_key`; never by path equality, so an unparked environment home binds only the migrated project's entities |
| `verdict_agent_owner` (`verdict_delivery.rs:145-155`) | `repo_path == delivery.project_path` | identity | `project_key == delivery.project_path` |
| `note_verdict_delivery_changed` (`verdict_delivery.rs:253-256`) | find project by `repo_path` | identity | `project_by_key` |
| `completion_authority.rs:185`, `activity.rs:124`, `deferred_delivery.rs:310,506` compare `tracker_project_path` with intent/task `project_path` | key == key | identity | unchanged once `tracker_project_path` returns the key |
| project-agent intent routing (`tracker/waiting_on.rs:110`) `project.repo_path == task.project_path` | path == task key | identity | `project_by_key(task.project_path)`; a uuid-keyed task's intent otherwise routes to no project agent |
| dependency-change publication (`tracker/waiting_on.rs:167`) `p.repo_path == path` over changed task keys | path == task key | identity | `project_by_key(path)`; otherwise `note_tasks_changed` is never called for a uuid-keyed project and its board never learns a dependency moved |
| worker/agent spawn cwd, `WorktreeManager::new(&source.path, …)`, base sync, `fs.list` roots | real paths | location | source paths and `home_path`; unchanged |
| `find_by_canonical_path` for project-creation dedupe (`projects/mod.rs:374,409`; `projects/lifecycle.rs:39,114` "project already registered") | path → existing project | neither | legacy `project.create` keeps returning the migrated project for its own home (compat, 2.1); the registry gains `environments_referencing(dir)` for the shared-directory checks of 1.4, and no new caller may use the path lookup for identity |
| MCP `list_projects`, `project.list` `path` field | display | location | `home_path`; M1.1 adds `project_key` to rows for clients that need identity |

**M0.2 implementation amendment (#1040):** prerequisite edges remain same-project,
as enforced by the store and ARCHITECTURE.md. The sibling dependency regression
checks cross-project refusal and notification isolation for a dependency within
the UUID-keyed project. Identity persistence also includes run/plan writes, SQL
project mappings, archived-worktree joins, background review sync, and parked
compaction/ownership inputs discovered by the full inventory.

M0 ends with a test per row (4, M0.2) rather than one test per function.
This table is the **seed** of a repository-wide per-usage audit, not its
result: M0.2 walks every reader of `repo_path`, `project_path`,
`find_by_canonical_path` and `tracker_project_path` and classifies each as
identity or location in a committed inventory
(`planning/v2/Environments Identity Audit.md`). A lexical grep that comes
back clean does not close the slice; every usage needs a written
classification, and every identity usage a test.

### 1.4 Shared source directories

Projects may list the same folder. Precisely:

- **Identity of a source reference** is its **canonical directory path**
  (`std::fs::canonicalize`), the same for a plain folder and for a Git
  checkout. Two linked worktrees of one repository are **two** directories
  and stay two sources; the common Git directory is **not** an identity.
- **Serialisation** reuses `SyncLock` unchanged: operations on two
  references to one directory, or to two worktrees of one repository, queue
  on the one canonical common Git dir, deduplicated by `acquire_many`
  (`source_sync.rs:437`). No project-keyed lock is added.
- **Overlap refusal becomes a containment refusal.** What stays refused,
  across every environment and every project's added sources on the device:
  a path that **contains or sits inside** another registered source (two
  sources must never include each other's files), a path inside the secrets
  vault, a managed clone destination overlapping a local source
  (`requests.rs:52,82,319`), and registering the **same remote twice** as two
  clones (`requests.rs:333`; share the directory instead of cloning again).
  What stops being refused: a path **equal** to an existing registered source
  (same canonical directory) **in another project or environment**. Within
  **one project's effective sources** (its environment's sources plus its
  added sources) a canonical directory may appear **once**: adding a folder
  the project already has, through the environment or its own list, is
  refused "already included in this project" before any write. The test
  `added_source_overlap_refused_across_projects` from the earlier draft is
  **dropped**; `source_overlap_refused_across_environments` keeps the
  containment cases and `added_source_duplicate_within_project_refused`
  covers the new rule.
- **Unlink removes a reference**, never files. `project.remove_source` and
  `environment.remove_source` delete that one record, exactly as
  `project_remove_source` does today (`requests.rs:475-516` touches no
  folder). The `remove_dir_all` at `requests.rs:147` is cleanup of checkouts a
  **failed creation** just cloned, not removal; it stays that, and the last
  reference to a managed clone is unlinked without deleting it.
- **Reference edits are local; repository mutations are shared.** Two kinds
  of change are kept apart:
  - *Editing a reference* (`update_source`: rename, repoint to another
    existing folder, base branch, sync flag; `source_update.rs:64-99`)
    changes **that one project's or environment's record** and the
    workspaces cut from it (`checkouts_cut_from`). It never moves a folder
    and never retargets a sibling's reference to the same directory; the
    sibling keeps pointing where it did. The worker-hold refusal
    (`refuse_moving_sources_under_workers`) stays scoped to the owner whose
    reference changes.
  - *Mutating the shared repository* (remote edits, fetches, managed clone
    operations) is one filesystem fact for every reference: it serialises on
    the directory's `SyncLock` as today and **invalidates every referencing
    project's views** (sources, workspace source status) when it completes.
  - *Sync status stays per reference.* Base-sync status is keyed by
    `project_id`/`source_id` today (`base_sync.rs:706,721,772`) and keeps
    that key: two projects may share one checkout and select **different
    base branches or sync settings**, so a directory-keyed conclusion would
    be wrong for one of them. Name, mount, base branch and sync selection are
    reference settings; only the Git lock is shared. Regression test
    `shared_directory_different_bases_keep_separate_sync_status`.
  No global "physical move" semantics and no global source registry are
  added by this feature; `environments_referencing(dir)` is a lookup over
  existing records.
- **Workspaces** cut from a shared directory are what they are today: a
  worktree or rift copy per workspace, each named by its own `source_id`.
- **Snapshot semantics** are unchanged and deliberately unexplained in the
  MVP UI: a workspace is cut from the shared original checkout's base branch
  at that moment. Explanatory copy waits for dogfooding (Stage 3).

### 1.5 Secrets: inheritance and override (Stage 2)

- New `GrantScope::Environment { environment_id }`. Readable by any current
  project agent, worker or workspace agent whose project belongs to that
  environment; **membership (`project.environment_id`) is revalidated on
  every read**, like project membership today (row 8); an archived project's
  agents are not current. Revoked when the environment is deleted (row 11
  analogue) and **re-checked when a project moves out of scope**: a project
  cannot change environment in this plan, so the only lifecycle events are
  environment delete and project delete/archive. Agent and Project scopes are
  unchanged; a project-scoped secret stays private to its line of work and is
  never copied to a project the wizard creates.
- **Override**: a project-scoped and an environment-scoped secret with the
  same `name` resolve to the project one for that project's agents. Nothing
  is merged; the environment value is not offered for that name there. The
  chip says "overrides environment". No override exists upward.
- Vault, sealing, audit and release path untouched; the scope enum, its
  validation (`model.rs:153`), list/revoke filters and `attached_secrets.md`
  wording change. Checklist delta in 7.

### 1.6 Isolation

`isolation` on the environment is the default for its projects; the project
override stays (`configured_isolation`, `bridge/src/app/config/mod.rs`).
Resolution: workspace → project → environment → device. Settings say
"Inherit environment setting" where they say "Inherit device setting" today.

### 1.7 Multi-device and device labels

Environments are facts of one bridge, like projects; ids are per device and
the SPA keys by `deviceKey(deviceId, environmentId)`. Nothing new crosses the
relay outside the sealed channel. **Device labels are MVP identity
information**: whenever the account has more than one device, every project
and environment row (wizard sections including Recent and search results,
switcher, settings, the final target summary and the progress screen) carries
the device name in the dim tag style the inbox uses (`dimDeviceHtml`), not
only on a name clash. A single-device account shows none. Offline devices
show "Offline" as today (`OFFLINE_TAG`).

### 1.8 Tasks stay on the project

Tasks belong to the project, numbered per project, exactly as the Tasks Spec
says. Two projects in one environment have two independent boards from #1.
The only change is the key (1.3): `tracker_project_path` returns
`project_key`; `store/tracker.rs:113`, `reviews.rs:235`,
`delivery_intents.rs:155,188`, `waiting_on.rs:188` are untouched. `tasks.*`
verbs keep `project_id`; `refuse_foreign_link` (`tracker/mod.rs:~640`) still
compares projects; MCP `list_tasks`/`create_task` are unchanged
(`mcp.rs:1112,1145`). A deleted project's key is never seen again; the only
way back to a board is **restore** of an archived project (1.9), not
re-adding a folder.

### 1.9 Archive (M3; settled by Zech, live-work policy in 5B.1)

`archived_at` on the project. Archiving keeps every task, review, run, plan,
comment and finished workspace under the same key. While archived:

- **Hidden** from wizard page 1, the switcher's grouped list, Recent, the
  inbox, and the workspace selector. **Found** by the switcher search and the
  settings page under "Archived", each tagged "Archived"; **direct links**
  (`#/project/<id>`, task and review links) open read-only with a banner and
  a Restore button.
- **Bridge-enforced read-only**: task, review, comment, assignment, source and
  workspace mutations on an archived project are refused with "P is archived;
  restore it first" (`error_code: project_archived`); MCP tools of its agents
  get the same refusal; its project agent and workers are not started;
  `workspace.create` into it is refused. Reads are unchanged.
- **Guarded**, not silent (**M3.4 implementation amendment, #1080**):
  `project.archive` enumerates blockers in `app/projects/archive.rs`,
  `app/runtime/sessions/registry.rs` and `store/project_archive.rs`:

  - Every unfinished or provisioning workspace, and pending journaled
    workspace creation for the project's key.
  - Live, starting or held worker/coordinator sessions (`agent_session_held`),
    and queued, in-flight or settling owner wakes.
  - Pending shell reservations and registered live shells in the project.
    Project-attributed retiring shells and retiring project-owner agents
    remain blockers until their retirement receipts are reaped, even after
    removal or replacement in the visible tab list. An exited registered
    shell awaiting row cleanup does not block by itself.
  - Needs-you tasks, unfinished waiting-on tasks, pending assignment intents
    (including failed delivery on an unfinished task), dependency-reconciliation
    markers, reviews awaiting a verdict, pending verdict/revision deliveries,
    unposted revision-failure notices and queued task notices.
  - Active deferred filesystem work (the global filesystem-job fence).

  The sorted refusal is `Cannot archive P: <blocker>; <blocker>`, with
  `error_code: conflict`, `retryable: false`, and `details.blockers` naming
  every action (for example, "finish or delete workspace W", "stop worker X",
  "wait for the closed terminal to exit (T)", "answer #12"). No work is
  implicitly stopped. A fresh user attempt after clearing a blocker is
  expected; `retryable: false` does not mean the Archive action stays disabled.
  The guard remains the chosen policy; the alternative of archiving while
  preserving attention visibility is listed in 5B.1.
- **Exceptions to read-only**, by name (**M3.4 implementation amendment,
  #1080**): the bridge already accepts `project.restore` and the existing
  `project.delete` on archived projects. M3.3 settings offers Restore while
  disabling Delete project on an archived project; deletion together with
  tasks remains Stage 2. **Reference
  maintenance is not user work on the project**: an environment source edit
  (3.3, M3.2) changes the environment, and the archived project's effective
  sources follow it as derived data, with no `project_archived` refusal and
  no workspace cut, since its workspaces are finished. Rows of an archived
  project still appear in `environments_referencing(dir)` for the
  containment and duplicate rules of 1.4.
- Migration default: nothing archived. Old clients see archived projects as
  ordinary rows (`project.list` keeps them with `archived_at`); an honest
  compatibility limit until the SPA floor passes M3's minor.

---

## 2. Wire and storage changes

### 2.1 Versioning and capability gating, per checkpoint

Each checkpoint bumps `API_VERSION` to the **next available minor at the
time it lands** (wire minors never skip); the numbers below are examples
assuming nothing else lands first. M1–M3 landed at the minors shown in the
table (through 3.39.0); Stage 2 rows remain planned. A capability is advertised
**only when its verbs and their consumer ship together**; nothing from a later checkpoint is
advertised early. Each bump updates `fixtures/api/versions.json`,
`fixtures/api/v1/session.hello.json`, `scripts/api-verbs-manifest.mjs`; each
new verb needs `fixtures/api/v1/<verb>.json` (`bridge/tests/api_contract.rs:332`),
an effect class in `bridge/src/api/effects.rs`, a `USER_ACTIVITY_VERBS` row
(`bridge/src/app/rpc.rs:49`) when it is a user write, a route row in
`bridge/src/http/routes/resources.tsv` with manifests regenerated by
`scripts/http-resource-manifest.mjs`; new params on existing verbs update
their fixtures because `params.strict` refuses undeclared params
(`api_contract.rs:972`).

| capability | checkpoint (example minor) | gates |
| --- | --- | --- |
| `environments` | M1 (3.37.0) | `environment.list`, `environment.create_project`, `project.discard_unused`, `environment_id`/`project_key`/`diverged`/`device` on project rows, `environments` in `board.list`, `environment_id` on workspace rows, journaled creation with `writes.progress` and `writes.status.progress` (2.3, 2.4) |
| `environments.create` | M2 (3.38.0) | `environment.create`, `environment.discard_unused` (inside the final Create flow only) |
| `projects.addedSources` | M2 (3.38.0) | `added_sources` param on `environment.create_project`, `project.*_source` on added sources, `added_sources[]` on rows |
| `projects.archive` | M3 (3.39.0) | `project.archive`, `project.restore`, `archived_at`, `project_archived` refusals |
| `projects.rename`, `environments.rename` | M3 (3.39.0) | `project.rename`, `environment.rename`, `environment.*_source`, `environment.set_isolation` |
| `secrets.environmentScope` | Stage 2 | `GrantScope::Environment` on `secrets.*` |
| `projects.deleteTasks`, `environments.delete` | Stage 2 | `project.delete {tasks}`, `environment.delete` |

SPA side: a persisted per-device support record `environment-support`
written at greeting (pattern `spa/src/core/sourceEditSupport.js:15-34`, called
from `publishGreeting`, `changeEvents.js:1017-1050`); `capabilitiesOf()`
(`bridgeApi/v1/index.js:157`) gains `environments: {list, createProject,
create, addedSources, archive, projectRename, environmentRename, secrets}`;
`NO_CAPABILITIES` (`changeEvents.js:125`) the same shape, all false. Views read the record from
cache, never the greeting. Each control appears only when its flag is true:
the "New environment…" row (M2), the Advanced folders section (M2), archive
and rename (M3), secrets (Stage 2).

**Old SPA, new bridge.** `project.list`, `project.create`, `workspace.create`
keep every field and param. `project.create` without `environment_id` makes an
implicit environment (name = project name, sources = given sources) and one
project; the old "Add project" sheet keeps working and yields a correct model.
**New SPA, old bridge.** Without `environments` the wizard lists projects
only (each is its own line of work) and picking one is `workspace.create`;
the Environments section and "New environment…" are absent. The old "Add
project" sheet (`newRepo.js`) is **retained as a capability-gated legacy
component**: it is reachable from settings only when the device lacks
`environments.create`, keeps its tests, and is deleted in S4.2 when the SPA
floor passes M2's minor. The settings page shows "Update Build on this
device to manage environments here" (pattern
`spa/src/sheets/projectSources.js:87-89`); nav renders from `projects` and
`workspaces` as today. Nothing is painted from connection state.

### 2.2 New and changed RPC verbs

New family file `bridge/src/api/v1/environment.rs`, registered with
`v1_method!`. Checkpoint in the first column.

| cp | verb | params | result | notes |
| --- | --- | --- | --- | --- |
| M1 | `environment.list` | `{}` | `{environments: [EnvironmentRow]}` | row: `environment_id, name, home_path, is_git, base_branch, sources[], isolation, isolation_effective, project_ids[], device` |
| M1 | `environment.create_project` | `{environment_id, name, workspace: {name?, isolation?}, added_sources?: [...] (M2), continues?: <operation_id>}` with frame `write.operation_id` | `{project: ProjectRow, workspace: WorkspaceRow}` | the wizard's Create for an existing environment: mints the project (`project_key = project:<uuid>`, `home_path` = environment home), then cuts the first workspace through `WorkspaceCreateWork` unpublished (2.3). One journaled command |
| M1 | `project.discard_unused` | `{project_id, creation_operation_id}` | `{}` | removes a project created by that attempt that has no workspace, task or agent session; otherwise refused naming what exists (2.3) |
| M2 | `environment.discard_unused` | `{environment_id, creation_operation_id}` | `{}` | the same for an environment created by that attempt that has no projects |
| M2 | `environment.create` | `{name?, sources: [{path\|remote, name?, base_branch?}], isolation?, project: {name, workspace, added_sources?}, continues?: <operation_id>}` with `write.operation_id` | `{environment, project, workspace}` | the wizard's Create for a new environment: environment, project and first workspace as **one** journaled attempt; never called from page 2 itself. `parent` as `project.create` has it (`board.rs:206`) |
| M3 | `project.archive` / `project.restore` | `{project_id}` | `{project}` | guards in 1.9 |
| M3 | `project.rename` / `environment.rename` | `{…_id, name}` | row | display only; one bounded line; never a folder |
| M3 | `environment.add_source` / `remove_source` / `update_source` / `sync_source` / `set_isolation` | as the `project.*` twins (`board.rs:206-240`) with `environment_id` | as today | forward-looking; shared-directory rules of 1.4 |
| S2 | `environment.delete` | `{environment_id}` | `{}` | refused while any project belongs to it; revokes environment secrets |
| S2 | `project.delete` | gains `tasks: "delete"` | | refused while a workspace is unfinished; a durable deletion record fences the project and captures owned cleanup before config removal, then removes history under its exact key (5B) |

Changed:

| cp | verb | change |
| --- | --- | --- |
| M1 | `project.list` | rows gain `environment_id, environment_name, project_key, diverged, device, open_tasks, total_tasks, workspaces, archived_at (M3), added_sources[] (M2)`; `sources` keeps listing effective sources; `path` is `home_path` |
| M1 | `workspace.list` / `workspace.get` | rows gain `environment_id` |
| M1 | `workspace.create` | optional `continues?: <operation_id>` when it retries the workspace step of a prior attempt (2.3); result shape unchanged. Fixture gains the optional param; `api_contract.rs` strict params updated |
| M1 | `board.list` | gains `environments: [EnvironmentRow]` beside `projects` (`fixtures/api/v1/board.list.json`) |
| M1 | `project.create` | unchanged shape; without `environment_id` makes an implicit environment |
| M2 | `project.create` | optional `environment_id`; `sources` are then added sources, `diverged: true` |
| M1 | `project.sync_source` | resolves the source from the project's effective sources, validates its captured path, and queues the existing project/source sync service (`{pending: true}`). It is not routed to a separate environment job; shared checkout mutations invalidate every referencing project's views, with sync status kept per reference |
| M2 (planned restriction) | `project.add_source` / `remove_source` / `update_source` | operate on **added** sources only; naming an environment source id is refused "source-N belongs to environment E; edit it there". Planned old-client compatibility for `update_source` routes an environment source edit to its owning record (5C.5). M1 still accepts legacy project source edits; it does not enforce this future restriction |
| M3 | every task/review/comment/workspace/source mutation | refused with `project_archived` on an archived project |
| S2 | `secrets.create/list/revoke/audit` | `scope` accepts `{environment_id}` |
| — | `tasks.*` | **unchanged** |

MCP tools (`bridge/src/mcp.rs`): no project arguments anywhere; scope comes
from the binding. `create_workspace` (`mcp.rs:1546`) stays "cut in your
project"; `list_tasks`/`create_task` stay "your project's board". Stage 2 adds
Project-surface `add_environment_source`/`remove_environment_source`
mirroring `add_project_source`/`remove_project_source` (`mcp.rs:1681,1693`),
and rewords `add_project_source` to "adds a folder to **this project only**,
making it diverge from its environment". No tool creates projects or
archives. Templates to reword: `bridge/templates/project_agent.md` (lines 1,
5, 15), `notes/task_tools.md`, `notes/workspace.md`,
`messages/attached_secrets.md:4`.

### 2.3 The creation contract (defined before any wizard code)

One **attempt** = one journaled command, identified by the request frame's
`write.operation_id` (`ARCHITECTURE.md:192`); no second id in the body. The
SPA mints it in `submitWrite` and persists the submitted params with it
(`writeOperations.js`), so a reload or another tab finds the attempt. The
bridge admits `environment.create_project` (M1) and `environment.create`
(M2) through the schema-18 journal: **this plan enables the executor for
exactly these verbs**, plus `workspace.create` when the wizard issues it in
fixed-project mode, and nothing else.

**Steps inside one attempt**, in order, each recorded as a sub-outcome on the
same command so `writes.status` for the one id reports where it stands:
`environment` (M2 only) → `project` → `workspace` (materialise each source;
`Copying <dir>` is one sub-step per directory, reported from
`materialize_source`). Config steps are the atomic temp-and-rename write
(`persistence.rs:86-104`); the workspace step is `WorkspaceCreateWork` with
`unpublished: true`.

**Outcomes** (the progress screen paints these from the write-operation cache,
3.2):

- *Done*: all steps terminal-ok; the result names environment, project and
  workspace ids; the SPA upserts them (`cacheCreatedWorkspace`,
  `createWork.js:47-52`, plus project and environment upserts) and navigates.
- *Failed at step k*: steps before k **landed and stay**. The workspace step
  cleans up its own candidate as it does today (rollback of the directories
  it made, removal of the private root); config steps are not undone. The
  outcome lists exactly what exists, and the offered actions depend on the
  **last successful step**:
  - *nothing landed* (the environment or project config step itself was
    refused): "Nothing was created: <refusal>". Back to the wizard with the **draft
    intact**; Create submits a fresh attempt.
  - *environment landed, project did not* (M2): the environment exists with
    **no projects** and is shown as such in settings and page 1 ("no
    projects yet"). Actions: **Resume**, which submits
    `environment.create_project` on that environment with the retained
    draft (name, workspace, added folders) as a new attempt; **Keep the
    environment**; **Discard the environment**, a narrow
    `environment.discard_unused {environment_id, creation_operation_id}`
    refused unless the environment came from that attempt and has no
    projects.
    Resume retains the landed environment and its creation operation id in
    the durable draft, including across reloads and further failures. A
    project created by Resume has its own creation operation id; each
    resource's discard action uses the operation that created that resource.
  - *project landed, workspace did not*: "Project P was created. Its
    workspace was not: <refusal>". Actions: **Try the workspace again**,
    a new attempt bound to the existing project (fixed-project
    `workspace.create`, never a second `create_project`); **Keep it**;
    **Discard this project**, a narrow `project.discard_unused {project_id,
    creation_operation_id}` refused unless the project was created by that
    attempt and has no workspace, task or agent session. These are the
    creation flow's narrow removals. Legacy Project settings still offers
    `project.delete {confirm: true}` for any project, with the confirmation,
    refusals and destructive scope described at M1. The new deletion flow
    with explicit task handling remains planned for Stage 2.
    Workspace retries retain the landed project and any environment created
    in this run in their durable SPA operation drafts, along with the original
    project-creation operation id for Discard. Every failed retry, including
    after reload, names those resources and shows the latest attempt's error
    with the same three actions.
  - *workspace candidate left behind* (rollback itself failed): the attempt
    is **not terminal yet**. It enters a non-terminal **reconciliation**
    state whose journal row persists the residual-resource evidence (the
    candidate's paths and what `rollback` reported), so a restart resumes
    cleanup from the record. While the reclaim path is actually running the
    progress shows "Cleaning up…" (2.4 progress, state `running`); when it
    stops without clearing the residue the progress shows **"Needs
    cleanup"** with the paths, and the attempt stays non-terminal until a
    later reclaim pass or the user clears them. No new provisioning attempt
    (Try again, Resume) is offered while residue is recorded, so a retry
    never cuts beside a half-made root. Only when residue is settled does
    the attempt reach its terminal outcome and `writes.confirmed` fires;
    `writes.confirmed` always means final.
  Every terminal retry or resume is a **new attempt with its own frame
  `operation_id`** whose journaled params carry `continues: <prior
  operation_id>`, so `writes.status` of either id shows the chain and the
  progress screen stays on one record. `creation_operation_id` in a discard
  body names the attempt that created the thing, distinct from the discard's
  own frame id.
- *Unknown*: the reply was lost (disconnect, timeout, bridge restart
  mid-step). The attempt stays **Checking…**; the SPA asks `writes.status`
  on reconnect and subscribes; the journal keeps an ambiguous external claim
  as unknown and **blocks a conflicting execution** of the same intent until
  reconciled (`ARCHITECTURE.md:1282-1289`). The user is never offered a retry
  that could create a second project; "Try again" appears only after the
  outcome is terminal. A bridge restart reconciles from its own records
  (config written? workspace reservation claimed? `recover_existing` path)
  and writes the terminal outcome.
- *Partial after cleanup failure*: when the workspace rollback itself fails,
  the outcome says so verbatim ("could not fully roll back…", as
  `workspaces/mod.rs:111-116` already phrases it), the workspace row stays
  visible as failed in the inbox, and **nothing claims a clean slate**.

**Cancel** means three different things and the UI says which: *Cancel* on
any wizard page before Create discards drafts and nothing exists; *Close* on
the progress screen detaches (the attempt continues; the inbox shows the
provisioning row; reopening the wizard shows "Still creating ‹name›…" with a
link); there is **no provisioning cancellation** in the MVP (a provisioning
workspace is handled by the existing failure/reclaim paths; a cancel verb is
5C.20).

Fixed-project mode (page 1 project row, nav footer, inbox "+") is today's
`workspace.create` with `write.operation_id`, same progress screen, one step:
the environment and project steps are absent from progress (not shown as
skipped), and the result is today's workspace row; no environment or project
is invented in the outcome. `continues` is the same optional param on all
three creation verbs (`environment.create`, `environment.create_project`,
`workspace.create`), so every retry path of 2.3 can link its predecessor;
each fixture carries it as optional.

### 2.4 Push events

The change bus (`bridge/src/changes.rs`) gains `Scope::Environment` and
`ScopeWire {kind: "environment", id}` (`changes.rs:351-421`).
`BoardLists {projects, workspaces, usage_limits}` (`changes.rs:609-614`) gains
`environments`, published on the board scope so `writePushedBoard`
(`spa/src/core/cacheSync.js:2068-2091`) learns a third list with its own
stamp. A creation attempt publishes project and workspace through the
existing board pushes as each step lands, so an old SPA tab sees them.

M1.2 implementation note: current `sync.device` bridges bypass the legacy
`readSyncLists` / `writePushedBoard` ownership path. Environments remain an
independent per-device cached list: both paths read `environment.list` only
with the current `environments` capability and apply carried board pushes.
Each read reserves a per-device `environment-list-order` admission token
before RPC admission; each carried environments array reserves one before
the body queue. Reservation is atomic across tabs and does not repaint the
list. Publication checks the latest token, current capability and lifetime
in the same transaction as replacing the list. No older admission can
overwrite a newer one, including queued pushes and overlapping reads.
This is admission order, not a server revision: a push admitted after a read
retires that read even if its eventual reply has slightly newer content.
The list may briefly be stale until the next admitted push/read publishes.
A failed newest read retains the last good list and never restores an older
admission's authority. Device journal projections do not own environments;
the existing projects/workspaces ordering is unchanged (follow-up #1037).
`environmentCache.js` exports `readCachedEnvironments(deviceId)` for consumers.

**Step progress is an additive wire change.** `writes.confirmed` is built
from a **terminal** outcome only (`confirmation`, `mutations.rs:328-345`) and
the SPA treats it as confirmation (`writeOperations.js`); intermediate steps
must never travel on it. M1 adds:

- bridge-side, in-memory per-attempt progress `{step, state, detail?}`
  (steps of 2.3; states `running`/`done`/`failed`), held by the executor
  while the attempt runs. It is not persisted: after a restart there is no
  in-flight step, only the reconciliation that writes the terminal outcome.
- `writes.status` result gains an optional `progress` field with the current
  step list while the command is non-terminal (fixture `writes.status.json`
  extended).
- a new push `writes.progress {operation_id, progress}` on an existing
  `writes.subscribe` subscription, sent on each step change and never for a
  terminal state (fixture `fixtures/api/v1/writes.progress.json`, manifest,
  effects class). Old SPAs ignore unknown push types.
- SPA `writeOperationCache` records gain `progress`; `writeOperations.js`
  stores it without changing the operation's `pending` state. Tests:
  `writes_progress_never_marks_outcome` (bridge), `writeOperationsProgress.test.js`
  (SPA: progress updates paint, confirmation still comes only from
  `writes.confirmed`).

`secrets.changed` carries the environment scope in Stage 2.

**M1.1 implementation amendment (#1043):** environment recovery uses the
capability-gated `environment.list` pull and ordinary carried board pushes.
There is no durable board aggregate in `sync.snapshot` / `sync.changes`:
the existing device journal has bounded per-entity rows (64 KiB), and its SPA
consumer does not accept a board family. Durable per-environment rows and their
consumer are deferred. Board lists continue to travel whole on the existing
push path; the existing HTTP SSE 1 MiB event limit rejects oversized frames.
Creation belongs to M0.4, and independent sibling task numbering to M0.2;
neither is part of the M1.1 read-wire acceptance tests.

### 2.5 Storage

**`config.json`** (written by `config_value_with_project`,
`persistence.rs:26-84`):

```jsonc
{
  "next_environment": 3,
  "environments": [
    { "id": "env-1", "name": "build", "next_source": 4,
      "sources": [ { "id": "source-1", "name": "build", "mount": "build", "path": "/home/z/Projects/build", "is_git": true, "base_branch": "main", "sync_base": true },
                   { "id": "source-2", "path": "/home/z/Projects/build-docs", ... } ],
      "isolation": "worktree" }
  ],
  "projects": [
    { "id": "proj-1", "environment_id": "env-1", "name": "build",
      "project_key": "/home/z/Projects/build",            // migrated: the old path key, unchanged
      "home_path": "/home/z/Projects/build",
      "added_sources": [], "isolation": null, "archived_at": null,
      // legacy fields kept so a rolled-back bridge still loads; removed in Stage 4
      "path": "/home/z/Projects/build", "base_branch": "main", "next_source": 3, "sources": [ ...effective... ] },
    { "id": "proj-7", "environment_id": "env-1", "name": "Environments plan",
      "project_key": "project:6f1c…",                       // new: never a path, never parsed
      "home_path": "/home/z/Projects/build",
      "added_sources": [ { "id": "source-3", "path": "/home/z/Projects/design-system", ... } ],   // same directory as env-2's source-1 below: cross-project sharing
      "isolation": null, "archived_at": null,
      "path": "/home/z/Projects/build", "base_branch": "main", "next_source": 4, "sources": [ ...effective... ] }
  ]
}
// a second environment, "design", whose home is /home/z/Projects/design-system (env-2, source-1): proj-7 references the same checkout
```

`build-docs` is already in proj-7's effective sources through env-1, so
listing it again under `added_sources` is refused "already included in this
project" (1.4).

Loading order: environments first, then projects; a project naming an
unknown `environment_id` is parked (existing path, `persistence.rs:69`) with
a reason, never dropped.

**M0.1 implementation amendment (#1033):** parked entries migrate immediately,
without requiring their folders to return; their raw source records survive the
migrating write. Migration retains the project isolation override as a legacy
copy and also copies it to the environment. Configuration staging includes
environment records and dependent project source projections,
so a failed write restores both without replacing runtime orchestrators.

**M2.2 implementation amendment:** `set_isolation` now changes only the project
override; `initialize_legacy_isolation` separately initializes both copies when
loading legacy project-only configuration. A project without an override uses
its environment's setting, then the device default. Clearing the project
override restores that inheritance without changing the environment or sibling
projects; the M0.1 behavior of updating both copies on a project edit no longer
applies.

**Migration of every existing project** (first load of a bridge with this
code, before anything is served, in `AppState` construction). Each existing
project becomes one project under one new environment derived from its
sources; nothing is merged:

0. Write `config.json.pre-environments` beside the config once (never
   overwritten by a later load).
1. For each project entry without `environment_id`: mint `env-N`; copy
   `name`, `sources`, `next_source`, `isolation` into the environment; set
   `environment_id`, `added_sources = []`, `project_key = <canonical home
   path as the tracker uses it today>`, `home_path = <that path>`,
   `isolation` retained as the legacy copy (see the M0.1 amendment above),
   `archived_at = null`. The project keeps its `name` and `id`.
2. Parked projects migrate the same way immediately, keeping raw source records.
3. One atomic write keeping the legacy project fields. Idempotent: a second
   load finds `environment_id` everywhere and does nothing.
4. Tasks, secrets, workspaces, runs, plans, scratch folders: **nothing
   moves**; every path-keyed value equals the migrated project's key (1.3).
   No `project_path` is reinterpreted; a path stays a path.

**Downgrade boundary, tested not promised.** M0.4 raised the store to schema
26. A binary supporting a schema below 26 refuses to open the store before
applying pragmas or DDL, and startup fails; downgrading across that boundary
is unsupported even before a sibling project exists. Restoring only a config
snapshot does not make that database compatible.

The frozen legacy config reader ignores `environments` and reads the legacy
fields: with no sibling, it loads the original projects. Once a **sibling**
exists it sees two projects whose `path` is the same folder, collapses them
into the first project and applies the later entry's source/isolation settings
and counter. It ignores the sibling's opaque key; that sibling's tasks,
scratch and runs are **not** guaranteed to stay bound or reachable.
This config-only result does not establish binary/store compatibility.
M0.1 added the test
`downgrade_with_sibling_project` against a frozen copy of today's loader
(old-version tests need frozen copies, not `git show main:`) to record what
the legacy loader actually does, and `docs/using.md` states the boundary
from that result. The supported recovery path across the schema boundary is
**roll-forward**: fix on the new binary. Restoring the one-time pre-upgrade
snapshot (`config.json.pre-environments`) is documented as **deliberate
abandonment** of everything registered since (new projects' names, keys and
source references; their tasks and scratch rows stay in the store under keys
no project claims), never as recovery. For recovery the bridge keeps the
**previous current config** as `config.json.previous` on every atomic write
(one generation), so a bad write on the new binary is undone without losing
later state. Any authorized binary rollback within a compatible store schema
uses `scripts/buildctl --env <env> bridge rollback` with the environment named,
as `AGENTS.md` requires.

**SQLite:** M0.4 changed the schema to **26** for creation write-ahead
effect evidence. Pending older creation records without that proof are marked
unknown (`store/mutations/creation.rs:5-20`); creation uses the journal introduced
in schema 18, with this additional migration. Stage 2 now plans **schema 27**:
`secret_records.grant_scope` is `CHECK (grant_scope IN ('agent','project'))`
with `grant_project_id` (`store/secrets/schema.rs:58-62`), so the environment
scope is a table rebuild with `'environment'` and `grant_environment_id`,
planned with a schema-26 fixture for the migration test;
`secret_final_bindings`/`secret_audit` (`schema.rs:110-141`) gain the
environment id. A newer store is refused by an older binary before any write
(`store.rs:368-386`). `archived_at` lives in `config.json`; the planned M2/M3
config fields do not require another SQLite version in this plan.

---

## 3. UX flows

Every screen paints from `build-cache` and `build-ui` first, then pulls
(`ARCHITECTURE.md` "Render from cache"). Each screen below names the cache
records it reads; none reads connection state. **Count text** everywhere is
one helper, `taskCountText(open, total)`: `"32 open tasks"` when `open > 0`,
else `"102 tasks"`, including `"0 tasks"`.

### 3.1 Create-workspace wizard


**#1085 implementation amendment (2026-11-09).** The creation wizard now has
one main page, "New workspace": search stays at the top, the target
list scrolls in the middle, and optional Name, Advanced, errors/progress and New environment…,
Cancel/Close and Create workspace stay at the bottom. Rows select in place
with a check mark and pressed state; a device-labelled summary and outcome
remain beside Name after filtering or scrolling. Preset targets omit the
picker. Advanced is always available and contains isolation and effective
directories, plus project-only added folders where supported; its expanded
body stays directly below the summary in the pinned footer and scrolls within
a capped height. The target list shrinks to keep the other controls visible.
Selected and device names use single-line ellipses with full-text titles, while
the separate Offline label stays visible. The summary outcome says "Uses this
project's task board" or "Creates a new task board" without repeating the name. Errors and retained progress scroll within three-line caps.
New environment… opens configuration
on page 2; Back preserves the draft and Use environment returns it as a
selectable, editable row on page 1 without creating anything. Saved page-3
drafts restore on page 1. Blank project names are refused by the bridge, so
the SPA derives the environment's name and the lowest free cached sibling
suffix (2, 3, …) before sending the existing creation command. This amendment
supersedes the three-page layout, required name, hidden fixed-project Advanced
and Next-button descriptions below; the durable creation/progress protocol
is unchanged.

Replaces today's "New workspace in P" modal (`spa/src/core/createWork.js:54-68`)
as the single creation entry point, opened from: the switcher's "New
workspace…" (`toolbar.js:853-869`), the inbox's "New project" button
(`inboxShell.js:358-363`, relabelled "New workspace"), the project view "+"
(`projectView.js:160-180`, fixed-project mode), the inbox block "+", the nav
footer, settings shortcuts (3.3), and Ctrl/Cmd-N. **Every page writes drafts
to `build-ui` only** (`uiAddress({view:"create-workspace", kind:"draft"})`);
**no mutating request runs until Create**. Read-only browsing and validation
(`fs.list` for the folder picker, remote URL shape checks) are allowed and
create nothing.

**Page 1, "Where?"** Reads `environments`, `projects`, `workspaces` for every
device from cache plus navigation visits (`navigationVisits.js`, ≤20).

- Search on top (fuzzy rank as the switcher, `toolbarModel.js:93`); results
  keep their section kind, device label and count text.
- **Recent**: targets interacted with in the last 24 h (`RECENT_AFTER_MS`,
  `inboxOrdering.js:3`), newest first, at most 5; interaction time =
  `max(visit to the target or a workspace in it, workspace last_activity_ms,
  workspace/project created_at)`, all in cache (`sessionSpans.js:16-24`;
  visits are per device profile, so recents differ per device: accepted,
  first-try recents do not depend on any cross-device sync). Project and
  environment rows look different (kind glyph, 3.5). Omitted when empty.
- **Projects**, first, under a persistent section helper **"Uses the
  existing task board"**: every non-archived project sorted by recency: kind
  glyph, device label, name, "E ›" prefix only when E has more than one
  project, "+K folders" when diverged, `W workspaces · <count text>`. The
  helper is visible before any selection, since selecting advances to page
  3. Picking one goes to page 3 in fixed-project mode.
- **Environments**, after, under the helper **"Creates a new task board"**:
  alphabetical: kind glyph, device label, name, `N projects · M workspaces`.
  Picking one goes to page 3. Search results keep each row under its
  section helper.
- Last row, M2 onward (gated by `environments.create`): **"New
  environment…"** → page 2.
- Keyboard (M1): ↑/↓ across sections, Enter picks, Esc cancels, `/` focuses
  search; first row focused when opened from the shortcut; focus returns to
  the opener on close; the dialog is `role=dialog` with a labelled title.
- Empty states: no projects yet omits Projects and opens on Environments; no
  environments and no projects anywhere shows "New environment…" (M2; in M1
  it shows "Add a project from settings first", honest) with one line "Build
  keeps your code in environments. Make one from a folder or a Git URL."
  Offline device: rows still show (cache) with the device name **and** an
  Offline tag beside it, so several offline machines stay distinguishable;
  picking one proceeds and the attempt fails at Create with the transport
  refusal.
- Old bridge: Projects only; no Environments section; a note "This device
  needs an update to create environments here".

**Page 2, "New environment"** (M2, from "New environment…" only). Folder
browser via `fs.list` or Git URL, device choice when more than one device,
optional name following the first folder, isolation (`newRepo.js:263-283`
logic, re-homed). **Draft only**: `environment.create` never runs here; the
only requests are read-only `fs.list` calls for browsing. Validation is
local (non-empty, URL shape). "Back" keeps the draft; "Next"
goes to page 3 with the draft environment selected and labelled "(new)".
Containment and duplicate-remote refusals arrive at Create and are shown
under the folder list on return.

**Page 3, "What are you working on?"** Title "New workspace in E" (or "in
E › P" in fixed-project mode). Final target summary line: kind glyph, device
label, "E" or "E › P", "(new environment)" when from page 2, then the
outcome repeated: "Uses P's task board · <count text>" in fixed-project mode,
"Creates a new task board" otherwise.

- **Name** (required when creating a project, 5B.4; optional in fixed mode):
  names the line of work and its first workspace; branch preview
  `build/<slug>` (`branchNamePreview`, `toolbarModel.js`).
- Isolation select, "Inherit E's setting".
- **Directories this workspace will contain**: read-only list of effective
  sources.
- **Advanced** (M2, gated by `projects.addedSources`), collapsed:
  "Also add folders to this project only" with the same folder/URL picker;
  each row says "shared with E" or "shared with P₂" when the folder is
  already a source elsewhere, or "new folder"; containment refusals arrive at
  Create. Absent in fixed-project mode (editing an existing project's folders
  is settings, 3.3).
- Buttons: **Back**, **Cancel** (discard drafts), **Create**. Create builds
  one request (2.3) and opens the progress screen. Loading, error and paint
  rules are the progress screen's.

Mobile (≤760 px, M1/M2): full-height sheet like `.tbmenu` (`shell.css:882-889`),
pages slide, 44 px rows, the folder browser is below the Git URL field on
page 2 because browsing is awkward on a phone. Motion and reduced-motion
refinement are Stage 3.

### 3.2 Progress screen

Opens on Create, painted from the write-operation record for the attempt's
`operation_id` (`writeOperationCache.js`) and `writes.subscribe` updates;
survives reload and appears in any tab that reads the same record.

- Steps as rows: "Creating environment" (M2), "Creating project", "Creating
  workspace" with sub-rows "Copying ‹dir›" per directory; states *waiting*,
  *running*, *done*, *failed*, *checking* (unknown). The final target summary
  from page 3 stays on top.
- *Done*: "Open workspace" (auto-opens after a short pause unless focus is
  elsewhere); the project appears in the inbox and switcher from the pushed
  board.
- *Failed*: the refusal text verbatim under the failed step; what landed is
  listed; actions per 2.3 (Try the workspace again / Keep it / Discard this
  project when allowed). Config-step failure before anything landed: "Nothing
  was created: <refusal>" with Back to the wizard (drafts intact).
- *Checking…*: "Build lost the reply; checking what happened" with no retry
  button; resolves on `writes.status`. Reconnect banner is the ordinary one.
- *Close* detaches (2.3). Keyboard: Esc = Close; focus lands on the first
  action when the outcome is terminal.

### 3.3 Settings: environments and projects

Account route `#/account/environments` (router table, `spa/src/core/router.js:1-15`),
linked from account settings (`views/settings.js`) and from device settings
where "Projects" is today (`views/deviceSettings.js:46`,
`deviceProjects.js:22-29`). Reads `environments` and `projects` for every
device from cache; per-device pulls via `pullDeviceProjection` (pattern
`deviceProjects.js:78-90`).

- **List** (route and read-only list in **M1.4**; shortcuts in **M2.2**;
  archive group and sheets in M3): one card per environment: kind glyph,
  device label, name, home path, `N sources · M projects · K workspaces`.
  Under it every project: kind glyph, name, "+K folders", `W workspaces ·
  <count text>`, "Settings…" (today's sheet until M3). An environment with
  no projects (a resumable partial creation, 2.3) shows "no projects yet"
  and a **Resume…** row (M2.2). An **Archived** group under each environment
  (M3), collapsed, headed "Archived (N)", with "Archived", "Settings…" and
  "Restore" per row. Footer **"New environment…"** opens
  the wizard at page 2 (M2.2). Per environment, **"New project in E…"** opens
  the wizard at page 3 with E preselected, and **"New project with extra
  folders in E…"** opens it at page 3 with Advanced expanded (both M2.2).
  Both are explicitly *creation* shortcuts and say "Creates a workspace" on
  the button's secondary line. In M1 the list is read-only: no footer, no
  shortcuts, and the legacy "Add project" sheet stays where it is today.
- **Environment settings sheet** (`sheets/environmentSettings.js`, modelled on
  `projectSettings.js`; M3): General ("Environment name", "Save name" →
  `environment.rename`; "Home path" read-only), Sources
  (`sheets/projectSources.js` cards against `environment.*_source`; a shared
  row says "also used by P₂"; "Add folder…" / "Add Git remote…"), Isolation
  ("Inherit device setting", "Git worktree", "Rift (copy-on-write)").
  **M3.4 implementation amendment (#1080):** those three sections ship in
  M3.3; Secrets and environment deletion controls remain Stage 2 and are
  absent, rather than disabled placeholders.
- **Project settings sheet** (`projectSettings.js`, amended): General: name
  ("Project name", "Save name", M3 → `project.rename`), "In environment E"
  link, device; Sources:
  "From E" (read-only, "Edit in E") and **"Added to this project"**
  (editable, `project.*_source`, M2; this is ordinary editing of this project
  and **never opens the wizard**); Isolation "Inherit E's setting"; Archive
  (M3) with the guard's refusal shown verbatim. **M3.4 implementation amendment
  (#1080):** Archive stays pressable after a refusal; only a pending request
  disables it, with a single-flight latch surviving cache repaints. Blockers
  can clear without the client knowing, so the next click asks the bridge
  again. Danger zone retains the existing Delete project action; deletion
  together with tasks is Stage 2.
- States: cached list paints at once; "loading…" only when the cache is
  empty; an error replaces only a still-empty list (`deviceProjects.js:87-89`);
  empty shows "No environments on this device yet". Old bridge: projects
  under "Projects (this device needs an update to use environments)" with
  today's sheet, and the legacy "Add project" sheet (`newRepo.js`) offered
  as the only way to add one; it is a capability-gated component until S4.2.

### 3.4 Top nav

Today: `tb-switch` chevron (#618) opens the global picker; the project crumb;
the local workspace selector (`toolbarRender.js:324-364`).

- **Switcher** (chevron unchanged) lists **projects**, grouped under
  environment headings: Recent (last 24 h, ≤5), then each environment as a
  non-interactive heading (kind glyph, device label, name) with its active
  projects by recency: kind glyph, name, "+K folders", unread pill. Search
  also finds archived projects, tagged. Row → project view. Footer "New
  workspace…" opens the wizard. Heading "Switch to"; search placeholder
  "Search projects or workspaces"; workspaces still match
  (`navigationPickerModel.js:8-14`).
- **Crumb**: kind glyph, then "E › P" with E muted and truncating first; when
  E has exactly one project the crumb shows P only. ≤760 px: glyph and P.
- **Workspace selector** (`tb-workspace`): every workspace of the project by
  **most recently interacted with** (`last_activity_ms` desc, then visit,
  then created), current first and marked, finished ones under a "Finished"
  fold (owned here, once), empty copy "No workspaces in P yet"; footer "New
  workspace in this project" (wizard page 3, fixed) and "Manage workspaces".
  Replaces today's `WORKSPACE_STATUS_ORDER`. Off a workspace route it reads
  "Workspaces ▾".
- Paint from `projects`, `environments`, `workspaces`, visits and
  `navigationUnread`; the 900 px pinned-inbox geometry (#631) is untouched;
  glyphs are 16 px and add no width beyond that.

### 3.5 Badges

Two layers, kept apart:

- **MVP identity information (M1)**: a **kind glyph** per row (environment:
  outlined rounded square; project: filled circle; workspace: outlined
  circle, all 16 px, current foreground colour, `aria-hidden`), the **device
  label** (1.7), and the existing unread `.badge` pill (`styles.css:471`).
  No colour tokens, no hashing: nothing to design or theme before first try.
- **Identity badges (Stage 2, recommendation 5B.3)**: hue from
  `hash(deviceKey(device, environmentId)) mod 12` over tokens `--badge-1…12`
  in both themes (`styles.css:164-237`; colours live only in `core/theme.js`),
  glyph = first two letters of the row's name; siblings share hue, differ by
  glyph. Replaces the kind glyph in the same slot, so no layout moves.

Placement (both layers): switcher rows, wizard rows and target summary,
progress screen, crumb, inbox block heads (`projectHeadHtml`,
`inboxProjects.js:248-264`) and flat rows (`projectTagHtml`, `inbox.js:964-967`),
workspace selector rows, settings list. The inbox status dot is unchanged.

### 3.6 Inbox

Grouping stays **by project** (`workspaceBlockFor`, `inboxProjects.js:78-102`),
which is now exactly the line-of-work grouping. M1: block heads read "E › P"
(E only when it has more than one project) with the kind glyph and device
label; blocks of one environment are adjacent (sort by environment, then
`byAnchor`); archived projects are absent (M3). "New project" becomes "New
workspace" (wizard); block "+" opens page 3 fixed. Owned here, once; 3.4 and
4 refer to this section.

---

## 4. Stages and checkpoints

Four stages. **Stage 1 (MVP) is four checkpoints M0–M3**; each names what
Zech can try, what is still unavailable, the failure/recovery checks that
must pass, and the gate before it is exposed. Every slice is one branch from
`dev`, independently mergeable, TDD, with fixture/version updates where it
touches the wire; bridge slices run `cargo test`/clippy/fmt, SPA slices
lint/test/build; gitleaks before push and semgrep before ready. Bridge
features ship **with their consumers** inside a checkpoint; a bridge slice
may merge first but its capability is advertised only in the slice that
completes the checkpoint.

### Stage 1, M0: foundation (private; not a dogfood milestone)

**Try:** nothing new; the migrated device looks and behaves exactly as
before. **Gate:** every migrated board, scratch folder, run and review
delivery resolves to the same project as before; restart recovery with a
sibling project present (created through the test harness) binds every run,
plan and scratch to the right project; a shared folder can be referenced by
two projects in tests. Wire unchanged; capability not advertised.

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| M0.1 | **Bridge: environment model + config migration** (`app/environments/`, `Project.environment_id/project_key/home_path/added_sources/archived_at`, `config_value_with_project`, load order, pre-migration backup) | every existing project loads as one project under its own environment; round-trips; second load is a no-op; `config.json.pre-environments` written once; parked projects never dropped; downgrade boundary established and documented | `config_migration_tests.rs`: fixture with 2 projects (one parked, one isolation override) → environments; idempotence; legacy reload; `next_source` continuity; `sources_for` = env ++ added; `migrated_project_key_is_home_path`; `migration_failure_leaves_config_untouched`; `downgrade_with_sibling_project` (frozen old loader) |
| M0.2 | **Bridge: identity audit** (every row of the 1.3 table: `project_by_key`, `tracker_project_path` → key, `tracker_task` by key, revision-dispatch and session-summary recovery by key, `waiting_on.rs:110,167` intent routing and dependency publication by key, committed per-usage inventory, `scratch_key(key)`, recovery by key then `home_path`, `bind_retained_entities` by key, verdict delivery by key, `task_project_maps`; attachment-store containment left untouched; stale `recovery.rs:328` comment fixed) | one test per row; a sibling pair on one path has separate boards, scratch, run bindings and verdict routing; a uuid-keyed task is found by `tracker_task`; a failed-review notice for a uuid-keyed project is delivered; legacy records bind to the migrated project; a record whose key resolves to no project is kept with "repo missing" semantics and no project is added | `sibling_projects_number_tasks_independently`; `tracker_task_resolves_uuid_keyed_task`; `project_agent_intent_routes_by_key`; `dependency_change_reconciles_for_uuid_keyed_sibling` (two siblings on one path; cross-sibling edges are refused; a dependency within the UUID-keyed sibling changes and only its board is notified); `revision_dispatch_recovers_for_uuid_keyed_project`; `session_summaries_rebind_by_key`; `sibling_scratch_dirs_differ_and_migrated_keeps_legacy_dir`; `recovery_binds_runs_by_key_not_path`; `recovery_never_adds_project_from_record`; `legacy_record_binds_migrated_project_only`; `verdict_delivery_routes_by_key`; `bind_retained_entities_ignores_sibling_path`; `attachment_containment_stays_in_attachment_store` (unchanged behaviour pinned) |
| M0.3 | **Bridge: shared directories** (reference identity = canonical path; containment refusal replaces overlap refusal; equal path allowed across owners, refused within one project's effective sources; unlink never deletes; reference edits local, repository mutations invalidate every referencing project; sync status keyed per reference) | two projects reference one folder; containment, vault and duplicate-remote cases still refused; a duplicate within one project's effective sources refused; removing one reference leaves the folder and the other reference, including the last managed reference; repointing one reference leaves the sibling's untouched; a fetch on the shared checkout invalidates both projects' views; two projects on one checkout with different base branches keep separate sync status; two worktrees of one repo remain two sources | `same_directory_shared_by_two_projects`; `source_overlap_refused_across_environments` (containment cases); `added_source_duplicate_within_project_refused`; `unlink_keeps_folder_and_other_reference`; `unlink_last_managed_reference_keeps_clone`; `repoint_reference_leaves_sibling_reference`; `shared_repository_mutation_invalidates_every_referencing_project`; `shared_directory_different_bases_keep_separate_sync_status`; `linked_worktrees_stay_distinct_sources` |
| M0.4 | **Bridge: journal executor for creation** (`environment.create_project`, `workspace.create` admitted by `write.operation_id`; in-memory step progress, `writes.status.progress`, `writes.progress` push; `continues` chaining; unknown retained; terminal outcome; `recover_existing` reconciliation on restart; `project.discard_unused` guards) | a retried operation id returns the recorded outcome; a crash between project write and workspace cut reconciles to a terminal outcome on restart without a second project; unknown blocks a conflicting intent; progress never marks an outcome; a continued attempt links to its predecessor in status | `create_project_idempotent_by_operation_id`; `crash_between_steps_reconciles_without_duplicate`; `unknown_outcome_blocks_conflicting_retry`; `workspace_step_failure_reports_landed_project`; `writes_progress_never_marks_outcome`; `continued_attempt_links_prior_operation`; `discard_unused_refused_when_project_used` |

M0.4 implementation note (#1042): additive creation verbs and progress fixtures
initially used the 3.36.0 wire contract; M1.4 has since advertised the
`environments` feature at 3.37.0. The existing recovery envelope has only
accepted and executing non-terminal states, so unknown/reconciling creation records project as
executing with their precise durable state in `creation.state`. This preserves
the existing client's finite summary contract. M0.2 identity routing is merged:
`tracker_project_path` returns the opaque project key. Discard still checks
both that key and the current routing key defensively, and refuses tasks and
live or retained agent sessions belonging to the project.

### Stage 1, M1: first try (dogfood: separate work on the same environment)

**Merged through M1.4 (wire 3.37.0). Try:** on the Build device, open the
wizard from the inbox, pick the "build" environment, name the work, watch
the progress screen, land in the
new workspace; its board is empty while #1028 and the rest stay on the old
board; the switcher shows both projects under one heading with device
labels; the workspace selector orders by recency. **Unavailable today:** new
environment setup in the wizard (the legacy "Add project" sheet still does
that the old way), wizard extra folders, archive, rename, the new project
lifecycle flow in environment settings, and environment secrets. These limits
are listed in `docs/using.md`. **Honest limitation:** every new line of work
stays in the inbox until archive arrives, unless the project is deleted.
**Delete project remains available** in Project settings for any project,
including one under an environment. It requires confirmation and is refused
while creation is pending, another filesystem operation is running, or a
workspace is locked or provisioning. Pending shell launches and in-flight
agent delivery under the project's source, workspace or run roots also refuse
deletion. It removes the project and its workspaces from Build, permanently
removing files and unsaved changes in managed workspaces, Build-cut run
checkouts and the project conversation's scratch folder. It deletes associated
runs and their conversations, legacy plans and their canonical documents, and
captures routed to the project; revokes project secret grants and agent
access; cancels pending task deliveries; and clears retained inbox activity
history. Tracker-task records remain stored with identities preserved before
rosters disappear; this does not keep the deleted project or its board
available. Original project folders and external checkouts are kept. The wizard's
**Discard this project** remains the narrow action immediately after a failed
attempt, guarded by that attempt's identity and absence of workspaces, tasks
and sessions. **Failure checks:**
transport cut during Create shows Checking… then the real outcome; a failing
workspace cut leaves a visible project with "Try the workspace again";
reload mid-progress re-finds the attempt. **Gate:** M0 merged; browser tests
for the wizard, progress and nav; #523/#618/#631 layout tests green; wire
bumped to 3.37.0 and `environments` advertised in M1.4.

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| M1.1 | **Bridge: M1 read wire** (`environment.list`, project/workspace row fields incl. `device` and counts, `board.list.environments`, board-scope push, fixtures, HTTP manifest; capabilities withheld until M1.4) | contract tests pass; legacy `project.create` makes an implicit environment; pull and carried board pushes use the same environment rows | `implicit_environment_from_legacy_project_create`; `environment_list_rows_match_fixture`; `board_list_includes_environments`; `board_push_publishes_environments` |
| M1.2 | **SPA: capability + cache plumbing** (`environment-support`, `capabilitiesOf().environments`, `NO_CAPABILITIES`, `environments` list, `writePushedBoard` third list, `readSyncLists` asks `environment.list` when supported, `taskCountText`) | greeting writes the record; a board push updates environments; without the capability nothing new is asked; count text rule | `environmentSupportWiring.test.js`, `cacheSyncChanges.test.js`, `namedCapabilitiesWiring.test.js`, `taskCountText.test.js` |
| M1.3 | **SPA: wizard pages 1 and 3 + progress screen** (`core/createWorkspaceWizard.js`, `createWorkspaceProgress.js`, target model with recents, drafts in `build-ui`, keyboard, mobile sheet, device labels, kind glyphs; replaces and **deletes** `createWork.js` modal; every caller switched; fixed-project mode) | sections and order; recents; empty states; old-bridge fallback; Create submits one `write.operation_id`; progress paints from the write-operation record across reload; outcomes Done/Failed/Checking render per 2.3 with the right actions; Cancel leaves no draft; Close detaches | `createWorkspaceWizardModel.test.js` (recents, order, empty, fallback, fixed mode, count text, device labels), `createWorkspaceWizardDom.test.js` (pages, keyboard, a11y names, mobile), `createWorkspaceProgress.test.js` (done, failed-with-landed-project, checking, reload, close), `createWork.test.js` removed with the modal |
| M1.4 | **SPA + bridge: top nav, inbox heads, read-only settings list, advertise** (switcher grouped with recents and device labels, crumb rule, workspace selector by recency with Finished fold, inbox "E › P" heads and adjacency, "New workspace" buttons; `#/account/environments` route with the read-only list of 3.3 and the old-bridge note; `environments` advertised; `API_VERSION` to the next minor) | grouping and order; selector order; crumb shows P alone for a one-project environment; settings list paints from cache with device labels and counts; layout tests unchanged | `toolbarModel.test.js` (`switcherModel`, `workspaceMenuModel`), `toolbarDom.test.js`, `navigationPicker.test.js`, `inboxProjects.test.js`, `environmentsSettingsDom.test.js` (read-only list, empty, old bridge), `test/browser/pinnedInboxToolbarLayout.test.js` unchanged |
| M1.5 | **Docs** (`ARCHITECTURE.md` Workspaces/RPC/Capability gating/cache, `docs/using.md` with the M1 wizard, actual limitations, legacy deletion and schema-26 downgrade boundary, Tasks Spec note on `project_key`; plan corrected for merged M0/M1) | sections describe merged code at the base SHA; future stages remain explicitly planned | n/a |

### Stage 1, M2: fresh setup (dogfood: a new environment end to end, Advanced folders)

**Try:** from the wizard, "New environment…", pick a folder or paste a Git
URL, name the work, Create, and watch environment → project → workspace;
then start a second line of work on it with an extra folder under Advanced
that another project also uses. **Unavailable:** archive, rename, environment
source editing in settings (M3), secrets. **Failure checks:** a containment
or duplicate-remote refusal returns to page 2/3 with drafts intact and
nothing created; a workspace failure after the environment and project
landed lists both and offers the M1 actions; a Git URL whose clone fails
leaves no environment (the clone is a source step inside the attempt and the
environment config step is refused before any write). **Gate:** M1 exposed;
`environments.create` and `projects.addedSources` advertised only here; the
legacy "Add project" sheet loses its new-bridge entry points in M2.2 and
stays as a capability-gated component for old bridges until S4.2 (2.1);
`environment.discard_unused` and the resumable empty environment of 2.3
work end to end.

**M2.4 documentation amendment (#1076):** `docs/using.md` describes the M2.2
wizard and M2.3 project settings, including the exact recovery actions. The
wizard's folder browser selects existing folders and performs only reads;
drafts are submitted by **Create workspace**. Advanced labels known references
as **shared with E** / **shared with P**, otherwise **new folder**; canonical
duplicate and containment checks still run at creation. Project settings uses
**From E** for read-only environment folders and **Added to this project** for
editable project-owned cards; adding a folder there never opens the wizard.
An empty environment has **Resume…** in the picker/settings; the failed
attempt's progress screen offers **Resume**, **Keep the environment** and
**Discard the environment**. Discard keeps source files. Archive, rename,
environment-folder editing in settings and environment secrets remain outside
M2; the limitations are listed separately so later slices can remove them.

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| M2.1 | **Bridge: `environment.create` as one attempt**, `environment.discard_unused`, `added_sources` on `create_project`, `project.*_source` on added sources with old-client routing, `diverged`, M2 fixtures; journal executor extended to `environment.create` | one operation id covers environment, project and workspace; refusals before any write leave nothing; a project-step failure leaves an environment with no projects that `create_project` can resume into; an added folder equal to **another** environment's or project's source is allowed, equal to one of the project's **own** effective sources refused, containment refused; env-source ids refused through project verbs | `environment_create_is_one_attempt`; `environment_create_refusal_writes_nothing`; `project_step_failure_leaves_resumable_empty_environment`; `environment_discard_unused_refused_with_projects`; `added_source_makes_project_diverged`; `added_source_may_equal_other_owners_source`; `added_source_equal_to_own_environment_source_refused`; `project_verbs_refuse_environment_source_ids`; `new_project_runs_recover_to_their_project` |
| M2.2 | **SPA: wizard page 2 + Advanced + settings shortcuts** (draft-only page 2 in `createWorkspaceEnvironmentDraft.js` / `createWorkspaceFolders.js`, using shared `sourceDraft.js` folder/URL validation and the read-only browser; Advanced section on page 3 with shared/new labels; progress gains the environment step and the per-step actions of 2.3; "New environment…" row, "New project in E…" and "New project with extra folders in E…" settings shortcuts, all gated by `environments.create`/`projects.addedSources`; the legacy "Add project" sheet stays, reachable only without `environments.create`) | page 2 issues only read-only `fs.list`; Create submits one attempt; refusals land on the right page with drafts; Advanced absent in fixed mode; settings shortcuts open the wizard preselected and say they create a workspace; empty environment shows "no projects yet" with Resume; legacy sheet hidden when the capability is present and shown otherwise | `createWorkspaceWizardPage2.test.js`; `createWorkspaceWizardModel.test.js` (advanced, shared labels, empty environment); `createWorkspaceProgress.test.js` (three steps, resume, discard environment); `environmentsSettingsDom.test.js` (shortcuts, legacy gate); `newProjectDom.test.js` kept |
| M2.3 | **SPA: project settings "Added to this project"** (editable cards via `project.*_source`; "From E" read-only; never opens the wizard) | adding/removing an added folder edits this project only | `projectSettings.test.js` |
| M2.4 | **Docs**: fresh setup, recovery, project-added and shared folders, isolation inheritance and M2 limitations in `docs/using.md`; reconcile the M0.1 isolation amendment | folder/URL setup and environment → project → workspace attempt documented; refusals, clone failure, partial outcomes, Resume and discard named as shipped; shared originals versus workspace copies and per-reference settings explained; project-only edits and isolation clearing leave siblings unchanged; archive, rename, environment-folder editing and environment secrets listed as unavailable | n/a (docs only; names and behavior checked against M2.2/M2.3 code) |

### Stage 1, M3: sustained use (MVP complete)

**MVP implementation complete through M3.4 (#1080).** M3.1 archive/restore
and M3.2 rename/environment source verbs are on dev. The M3.3 settings
consumer merged on dev at `750e70576648b05c73e5cff3ae162e82468a2518`.
`API_VERSION` is 3.39.0 and advertises `projects.archive`, `projects.rename`
and `environments.rename`. This records implementation and documentation
completion, not acceptance on dev: the seven Stage 1 journeys below remain
**pending manual execution on dev** against a migrated device.

**M3.4 documentation amendment (#1080):** `docs/using.md` now covers archive
blockers and reattempts, settings/banner Restore, display-only names bounded
to one line and 200 bytes, and environment General/Sources/Isolation versus
project-only folders. The M2 archive/rename/environment-folder limitations
are removed; the Stage 2 limitations below remain. The Terminology Audit
records the settled M3 labels; §1.9 and §3.3 follow the implemented guard and
single-flight Archive behavior. `ARCHITECTURE.md` describes wire publication
in terms of `API_VERSION` and consumer capabilities, now released at 3.39.0
with the merged M3.3 consumer.

**Try:** live on it for a week: archive finished lines of work and see them
leave the inbox and wizard, restore one from settings, rename a project and
an environment, edit an environment's folders in settings, find an archived
project from the switcher search and open its old tasks read-only.
**Unavailable:** environment secrets, environment delete, project delete
with tasks, MCP environment tools, identity badges (Stage 2). **Failure
checks:** archive refused with named blockers; an archived project refuses
mutations from the UI, MCP and agents with one error code; restore returns
everything. **Gate:** M2 exposed; `projects.archive`, `projects.rename`,
`environments.rename` advertised only here.

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| M3.1 | **Bridge: archive/restore** (`archived_at`, guards of 1.9, `project_archived` refusals across task/review/comment/source/workspace verbs and MCP, agents not started, rows kept on the wire with `archived_at`) | guards enumerate blockers; every mutation path refuses; reads work; restore clears; migration leaves none archived | `archive_refused_names_each_blocker` (workspace, worker, coordinator turn, pending intent, needs-you task, pending verdict); `archived_project_refuses_mutations_on_every_path` (one assertion per verb family + MCP); `archived_project_agent_not_started`; `restore_restores_everything` |
| M3.2 | **Bridge: rename + environment source verbs** (`project.rename`, `environment.rename`, `environment.*_source`, `environment.set_isolation`; one-line bounded names) | display-only renames; shared-directory rules apply to environment sources | `environment_rename_keeps_ids_and_paths`; `rename_is_one_bounded_line`; `isolation_resolution_order` |
| M3.3 | **SPA: settings page full** (environment sheet General/Sources/Isolation, project sheet rename/archive, Archived group with Restore, search tags, read-only banner on archived project/task/review views, hidden from wizard/switcher/inbox/selector) | every list hides archived rows except search and settings; banner + Restore; refusals verbatim; Archive pressable after refusal and disabled only during its single in-flight request | `environmentsSettingsDom.test.js`, `projectSettings.test.js`, `archivedProjectViews.test.js`, `createWorkspaceWizardModel.test.js` (archived hidden), `toolbarModel.test.js` (search tag) |
| M3.4 | **Docs complete (#1080)**: MVP implementation complete; archive/restore, rename, environment settings and remaining limits in `docs/using.md`; Terminology Audit and architecture amended | labels/refusals checked against M3.1–M3.3 merged on dev at wire 3.39.0; §1.9/§3.3 match the implementation; Stage 1 manual dev acceptance still pending | n/a (docs only) |

**Stage 1 done when** each of these journeys runs on dev, by hand, against
a migrated device:

1. From the inbox: wizard → existing environment → name → Create → the new
   workspace opens; its board is empty and the sibling's board is untouched.
2. From the inbox: wizard → "New environment…" → folder or URL → name →
   Create → environment, project and workspace appear in order.
3. A second line of work on that environment with an extra folder under
   Advanced that another project also lists.
4. The transport is cut during Create: the screen shows Checking… and then
   the true outcome, with the right per-step actions.
5. Archive a project: it leaves every list, is found by search and settings,
   opens read-only from a direct link, and Restore brings it back whole.
6. An old SPA tab on the same bridge still lists and creates projects.
7. `project.list` shows every migrated project with its `environment_id`
   and every one of its old tasks.

### Stage 2: full feature

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| S2.1 | **Bridge: environment secrets** (`GrantScope::Environment`, planned schema 27 rebuild, membership revalidated per read incl. archived = not current, revoke on environment delete, name override, `secrets.*` scope, `secrets.changed` scope, `secrets.environmentScope`) | readable by agents of every project in the environment and none outside; project secret of the same name wins; a sibling never reads another project's secret; archived project's agents refused; deletion revokes | `environment_scoped_secret_readable_across_projects`; `project_secret_overrides_environment_name`; `sibling_project_cannot_read_project_scoped_secret` (one per mechanism: release, list, attach, audit); `archived_project_agent_not_current_for_secrets`; `environment_delete_revokes_environment_scope`; leak sweep row 14 rerun |
| S2.2 | **Bridge: environment delete, project delete with tasks** (`environment.delete`, `project.delete {tasks}` accepted on archived projects too, parked-project unpark migration) | environment delete refused with "Delete its N projects first" while any project, archived or not, belongs to it; project delete refused while a workspace is unfinished, otherwise removes tasks, reviews and intents under its key only; journal-first, config-first deletion survives partial failure and restart using an immutable plan; pending deletions are fenced until completion or safe abandonment | `project_deletion_journal_failure_restart_and_retry`; `project_deletion_fences_rpc_mcp_and_registration_after_config_removal`; `project_deletion_history_and_phase_commit_atomically`; `project_deletion_retires_sessions_before_config_and_on_same_process_retry`; `project_deletion_history_checkpoint_rollback_preserves_real_review_pins`; `project_deletion_inventory_does_not_block_unrelated_requests`; `project_deletion_provisional_claim_survives_sibling_publication_after_abandonment`; `project_deletion_unavailable_pin_repository_retains_receipt_until_restart`; `project_delete_missing_source_completes_with_owed_pin_receipts`; `project_deletion_return_trip_drains_committed_pin_receipts`; `project_deletion_cleanup_allows_sibling_rename_and_workspace_delete`; `project_deletion_admission_is_sql_free_with_and_without_fences`; `project_deletion_return_trip_abandons_readded_registration`; `project_deletion_return_trip_abandons_changed_history_and_workspace`; `environment_delete_post_success_restart_retry_is_idempotent`; `environment_delete_refused_while_projects_exist` (incl. archived); `project_delete_removes_its_tasks_only`; `archived_project_accepts_delete` |
| S2.3 | **Bridge: MCP** (`add_environment_source`/`remove_environment_source`, reworded `add_project_source`, templates) | project agent can add a folder to its environment; a Coding-surface agent has no such tool | `mcp_project_surface_lists_environment_source_tools`; template snapshot |
| S2.4 | **SPA: secrets in environment settings + composer scope** (#1006 composer "This environment"; "overrides environment" chips) | create/list/revoke; old bridge hides the scope | `secretsWire.test.js`, `environmentSettingsSecrets.test.js` |
| S2.5 | **SPA: environment delete and project delete in settings** | refusal reasons verbatim; confirm names task and workspace counts | `environmentsSettingsDom.test.js`, `projectSettings.test.js` |
| S2.6 | **SPA: identity badges** (tokens, `identityBadgeHtml`, replaces kind glyphs in place) | identical hue across tabs; siblings share hue, differ by glyph; contrast in both themes | `identityBadge.test.js`, theme token test |

#### S2.2 approved recovery deviation (#1096)

Partial-failure review found that deleting history before checkout/config cleanup
could leave a registered project with its history permanently gone. Both project
delete modes now use a per-project record in the existing schema-26
`mutation_commands` JSON columns. No schema, wire-version or capability change
belongs to this deviation. The FULL writer transaction records exact project ID,
project key, task-deletion mode, the immutable owned path/checkout (including exact backend registration name) and entity-ID
plan, file evidence, scoped history fingerprint, phase and completion tombstone
before destructive effects. Retries never derive a new cleanup plan from config.

Ordering is provisional per-project fence, off-lock evidence/record admission,
scoped session retirement and wait, config removal, owned checkout/private-marker cleanup,
history/receipt commit, pin release, completion. Every attempt, including a
same-process resume, reconstructs live project/workspace agents and terminal
sessions from the immutable recorded IDs/owned paths and the current session
registry. Shared source paths never widen agent ownership to a sibling. A failed
or timed-out retirement leaves the plan pending with config/files/history intact.

Task/review/intent deletion, the history phase checkpoint and exact pin-release
receipts (repository/ref/expected OID, ownership-marker bytes when readable, and
exact deferred snapshot descriptors for unavailable metadata) commit in one
writer transaction. That transaction performs no irreversible Git/file effect.
Pin release is a separate idempotent post-commit phase; missing refs/markers are
success and failures retain the receipt for retry. Receipt draining is owed work
independent of plan abandonment: startup retries it even on terminal records,
and durably clears receipts only after successful release. A failed drain logs
the debt, preserves its record against collection, and retries at next startup.
A temporarily missing or unopenable repository does not block deletion: known
refs and exact task/snapshot/directory metadata remain owed on the completed
tombstone. Only a successful repository open can prove recorded refs absent;
startup resolves deferred ownership markers only within those captured snapshot
namespaces. There is no missing-and-unregistered discard; permanent removal may
retain a small receipt indefinitely. Other pin errors (changed refs/markers or
Git locks) preserve the existing refusal/retry contract.
A rollback retains history and all pins; a crash after commit may leave stray pins. Legacy mode keeps task
history and retires delivery authority.

Request validation, ID/path capture and the provisional fence are in-memory
work under the app mutex. Inventory walks, hashing, retry evidence, record and
phase writes, config IO, cleanup and final device-registry reconciliation run on
chained deferred blocking stages.
Private workspace marker paths are captured and inventoried explicitly; memory
publication never unlinks them. Older records default to no additional marker
authority. The final response/confirmation contract remains unchanged. Each continuation
checks its per-project claim before applying memory results. An off-lock failure
clears the provisional fence unless a durable pending record exists. Provisional
plans live in a separate claim map: only their checked owner may hand off to its
journal or release them after failure. Terminal/abandoned records and concurrent
sibling publication cannot drop that authority. Unrelated
reads/project work proceed while inventory or cleanup is paused. A config-write
reservation covers only its separate synchronous config-file write, capturing
the latest config after inventory/retirement and releasing before SQL checkpoints
or long cleanup. Deletion stages use project fences instead of the global
filesystem-job exclusion, so sibling renames/config writes and unrelated workspace
creation/deletion continue. Source references overlapping captured cleanup paths
remain refused by the scoped fence. Both deletion and archived-task admission use a
memory ownership index seeded before sharing AppState and updated with committed
task publications; admission performs no SQLite lookup. Startup
checks evidence during owned-state construction, before the shared mutex exists.
Every unfinished record fences RPC writes/assignments, workspace creation,
task reads and MCP use and refuses registration under its ID/key. After config
removal only a matching record authorizes the original delete; mismatched
identity or task mode is refused. Sibling history and source folders survive.

Startup performs no owned-file or history cleanup; committed pin-release receipts
are drained independently of pending-plan recovery. It logs `deletion-pending`, the
project ID and the retry instruction. A config failure leaves the project listed
but fenced, with config bytes and history intact. Once config is removed the
project disappears from ordinary project lists; requests to its captured
addresses return a deletion-pending error. Retry through the existing RPC:
`project.delete {project_id, confirm:true, tasks:"delete"}` (omit `tasks` for
legacy mode). Successful repeated project deletes use the retained completion
tombstone; absent environment deletes are harmless idempotent successes, across
restart as well. Pending records are never collected. Completed/abandoned
records with drained receipts are collected at startup and completion: retain at
most 256 and at most 30 days. Records with undrained pin receipts are exempt
until the debt is durably drained. An expired project tombstone no longer
authorizes a retry.

**Binary rollback boundary:** a previous schema-26 bridge ignores the internal
journal's unknown `deletion_pending`/`deletion_complete` states without decoding
or executing them. Config is removed before destructive cleanup, and SQL history
is deleted atomically, so rollback neither crashes nor exposes a half-deleted
history set. During the window from config removal to tombstone, only under a
binary rollback, two residuals are accepted: `workspace.list` can show remaining
manifests, and reopening the same legacy path can reattach the history that has
not yet been cleaned (an un-deletion). The older bridge cannot honor a fence it
does not implement; this does not authorize loss of new user work.

On the return trip, startup checks registration, scoped history and the remaining
owned files. A re-added registration, changed history, or new/changed owned file
abandons and tombstones stale owned-file/history authority. Committed pin
receipts are independent owed work: recovery drains them regardless of
abandonment and never collects an undrained receipt. If no
registration remains, it restores the captured project registration (and its
captured environment registration if rollback removed that too) so preserved
work is usable. Preserved workspace manifests keep their original directory identity. A fresh
delete can capture those workspace IDs through the abandoned record's exact
project-key link, provided no other project has taken their old ID.
The user then deletes again to capture fresh authority. Partial
cleanup may remove captured files; that alone does not invent authority over new
files. New sources occupying captured roots also prevent stale cleanup.

### Stage 3: polish (after dogfooding, informed by it)

| # | slice | acceptance | TDD tests |
| --- | --- | --- | --- |
| S3.1 | Explanatory copy: what a workspace is cut from, "shared with P₂" tooltips, first-run hint on the wizard, empty board copy | copy approved by Zech | DOM tests |
| S3.2 | Motion: page slides, progress transitions, reduced-motion; a11y audit of dialog and progress | audit passes; ≤760 px screenshot test | browser layout test |
| S3.3 | Environment view (`#/environment/<id>`: projects as cards with count text, sources); project view "Environment" link | | router + view tests |
| S3.4 | Cross-device recents reconciliation (visits merged by account) | | model tests |
| S3.5 | Terminology pass (`docs/using.md`, `ARCHITECTURE.md`, Terminology Audit) | | n/a |

### Stage 4: cleanup of old systems

Dead UI goes **with its replacement** (done in M1.3 for the create modal;
the "Add project" sheet's new-bridge entry points in M2.2). What remains is
version-gated compatibility,
removable once the SPA floor (`SPA_API_RANGE`, `spa/src/core/bridgeApi/index.js`)
passes the named minor and the desktop app ships that SPA.

| # | what goes | when safe | test |
| --- | --- | --- | --- |
| S4.1 | Legacy project fields in `config.json` (`path`, `base_branch`, `next_source`, `sources`); `Project.repo_path/base_branch/next_source` replaced by accessors | after one release with no downgrade need (Zech's call); loader reads them one more release, writer stops | migration test drops legacy fields and reloads |
| S4.2 | `project.create` without `environment_id` (implicit environment) → refused "name an environment"; the legacy "Add project" sheet (`newRepo.js`) and its old-bridge gate deleted from the SPA | SPA floor ≥ M2's minor | fixture flips; `newProjectDom.test.js` removed |
| S4.3 | Old-client compatibility routing of `project.update_source` to environment source records; `project.sync_source` continues through the project/source sync service | SPA floor ≥ M2's minor | fixture flips |
| S4.4 | `WORKSPACE_STATUS_ORDER` in `toolbarModel.js` | M1.4 merge (replaced in the same slice) | removed with M1.4 |
| S4.5 | `PersistedRun/PersistedPlan.project_path` renamed `project_key`; `schema.rs:226` comment reworded | any time after M0 (rename only) | entity round-trip |
| S4.6 | `config.json.pre-environments` backup deleted by the bridge | after S4.1 | |

---

## 5. Oversights and open questions

### 5A. Settled by Zech

1. Tasks belong to projects; many projects share an environment and differ by
   their boards.
2. Page 1 lists existing projects first, then environments; picking a project
   adds a workspace to it.
3. Workspace-first wizard with "new environment" as a page of the same flow;
   implicit project; settings page; two-part nav; badges in nav and inbox.
4. Projects can be archived ("Yes we should have archiving").

### 5B. Our recommendations, Zech's calls (each with the plan's current answer)

1. **Archive policy for live work.** The plan ships guarded
   archive in M3 and refuses while agents, coordinator turns, provisioning,
   pending intents or attention (Needs you, pending verdicts) exist, naming
   each blocker. Alternative: archive anyway and keep attention items visible
   in the inbox under an "Archived" head. *Recommend* the guard: hiding
   active work silently is the failure to avoid, and the blocker list is a
   to-do list the user understands.
2. **Tasks deleted with the project** (Stage 2 `project.delete {tasks}`).
   With archive available, delete is for mistakes. *Recommend* delete removes
   the project's tasks, reviews and intents; the confirm names the counts;
   archive is the default suggestion in that dialog.
3. **Identity glyph design** (S2.6): hue per environment, two-letter glyph,
   filled/outlined shapes. MVP ships kind glyphs and device labels so this
   can wait. *Recommend* as drawn in 3.5.
4. **Required project name.** *Recommend* required when creating a project
   (a project named after a branch slug makes the inbox unreadable), optional
   in fixed-project mode; editable later (M3).
5. **The word "environment"** collides with `SecretEnvironment {Dev, Prod}`
   (`bridge/src/thread/secrets.rs:14-33`), `buildctl --env` and the secret
   runtime CLI. *Recommend* renaming the secrets channel to **`stage`**
   (`SecretScope.stage` exists, `api/v1/secrets.rs:21-25`) in a mechanical
   slice before M1, or choosing another user-facing word ("space",
   "codebase"). The plan says "environment" until told otherwise.
6. **Vocabulary**: "project" stays the user-facing word for the line of work
   (crumb, inbox head, settings, board, and the wizard's own outcome copy:
   "Creates a new task board" sits under a "project"); the wizard's page-3
   title asks "What are you working on?" rather than "Name the project".
   Record in the Terminology Audit (#537).

### 5C. Recorded answers (ours; overturnable)

1. **Diverged project in the wizard**: picked as itself, "+K folders"; not a
   template; no overlap relaxation needed because equal paths are simply
   shareable now (1.4).
2. **What "recent" counts as**: `max(visit, workspace last_activity_ms,
   created)` within 24 h, SPA-side from cache; agent-only activity counts;
   per-device visits accepted for the MVP.
3. **Environment rename**: display-only; project names do not follow; ids,
   keys and roots unchanged.
4. **Secrets widening** (Stage 2): a project-scoped secret must never reach a
   sibling by any path; one negative test per mechanism; project-wins only;
   no "move secret up" (never re-seal a value).
5. **`project.create` compatibility**: implicit environment; old-client
   compatibility for `update_source` on environment source ids is planned until
   S4.3. `sync_source` queues the current project/source service, not an
   environment-owned sync job.
6. **Sources added to project vs environment**: the UI defaults to the
   environment; "this project only" is the Advanced section at creation and
   the "Added to this project" cards in settings; MCP `add_project_source`
   says it makes the project diverge.
7. **Multi-device**: per-device environments; same name on two devices is
   two environments, told apart by the device label (now always shown with
   more than one device). Cross-device "same remote" identity out of scope.
8. **MCP**: `create_workspace`, `list_tasks`, `create_task` unchanged; no
   `create_project`/`archive` tools; environment source tools in Stage 2.
9. **Inbox grouping**: per project; "E › P" heads only when E has several
   projects; environment adjacency; no block per environment.
10. **Mobile**: wizard and progress as full-height sheets in M1/M2; Git URL
    above the browser on page 2; motion polish Stage 3.
11. **Compaction and project agents** stay per project; the environment has
    none.
12. **Several projects on one path**: handled by the per-usage identity
    audit (1.3, M0.2); the risk that motivated it (a sibling's runs binding
    to the wrong project) has a named test.
13. **No merging of existing projects**: one environment per existing
    project; "move project to another environment" out of scope.
14. **Secrets schema rebuild** stays Stage 2; the MVP ships no secrets change.
15. **Wire naming**: `environment.create_project` and `environment.create`
    as single journaled attempts rather than client-chained verbs: one
    operation id, one audit line, sub-outcomes for progress.
16. **Count text**: one helper, `N open tasks` else `T tasks` incl. `0 tasks`.
17. **Shared source identity** is the canonical directory, never the common
    Git dir; linked worktrees stay distinct; `SyncLock` reused for
    serialisation only.
18. **Partial outcomes**: config steps that landed stay and are reported;
    only the workspace candidate is rolled back; the offered actions depend
    on the last successful step (retry, resume into an empty environment,
    retry the workspace, discard); every retry is a new attempt linked by
    `continues`; discards name the creating `creation_operation_id`;
    unknown never retries into a duplicate; `project.discard_unused` (M1)
    and `environment.discard_unused` (M2) are the creation flow's narrow
    removals. Legacy `project.delete` is still exposed by Project settings
    (M1); the Stage 2 flow adds explicit task handling.
19. **Downgrade boundary**: M0.4's schema 26 is refused by binaries supporting
    an earlier schema, even before a new project exists. Roll-forward is the
    supported path across that boundary. The frozen legacy config loader
    collapses siblings; its result is not binary/store compatibility. The
    pre-migration snapshot is deliberate abandonment, and
    `config.json.previous` is the recovery backup (2.5).
20. **Provisioning cancel**: not in MVP; a `workspace.cancel_create` that
    turns an unpublished candidate into a terminal failed outcome is a
    candidate for Stage 2 if dogfooding asks for it.
21. **Secrets lifecycle in Stage 2** needs environment membership checks and
    revocation on environment delete and archive, not the identity split
    alone (S2.1 tests).
22. **`proj-N` durability**: ids are persisted and restored
    (`persistence.rs:49`, `projects/mod.rs:426`); the recovery comment that
    says otherwise (`recovery.rs:328`) predates that and is amended in M0.2.
    Identity still uses `project_key`, not the id, because a key must survive
    the project's deletion in historical rows without being reissued, and
    `project:<uuid>` can never collide with a reissued `proj-N`.

---

## 6. Risks

- **Identity by path survives somewhere.** Twenty-plus usages; one missed
  comparison binds a sibling's run or verdict to the wrong project.
  Mitigation: the per-usage table (1.3) with one test per row in M0.2, and a
  grep gate in M0.2's CI step: no new `repo_path ==` or
  `find_by_canonical_path` outside the allowlisted location uses.
- **A second general job system.** Mitigation: the creation contract is the
  existing journal envelope plus `WorkspaceCreateWork`; M0.4 enables an
  executor for named verbs only.
- **Claiming rollback we do not have.** Mitigation: outcomes list what
  landed; unknown stays unknown; tests `workspace_step_failure_reports_landed_project`
  and `unknown_outcome_blocks_conflicting_retry`.
- **Duplicate projects from retries.** Mitigation: one operation id per
  attempt; retry of the workspace is fixed-project and never `create_project`;
  journal fences conflicting intents.
- **Config migration on a live device.** Mitigation: backup first, atomic
  write, idempotence, parked never dropped, downgrade boundary tested and
  documented.
- **Shared-directory edge cases** (move under holds, sync state, managed clone
  removal). Mitigation: M0.3's named tests; containment refusal unchanged.
- **Capability drift**: advertising a flag before its consumer ships.
  Mitigation: the flag constant lands with the bridge slice but is added to
  `FEATURE_CAPABILITIES` only in the checkpoint's closing slice (M1.4, M2.2,
  M3.3); `api_contract.rs` pins the advertised set per version.
- **Many small projects.** Intended; archive (M3) and the Finished fold carry
  it; until M3 the plan says so in the UI.
- **Nav geometry regressions.** Fixed 16 px glyphs; #523/#618/#631 tests
  stay green as acceptance for M1.4.
- **Word collision with the secrets channel** (5B.5). Decide before copy and
  verb names land in M1.
- **Scope creep into secrets.** No secrets change before Stage 2; S2.1 carries
  its own checklist rows and negative controls.

---

## 7. Security checklist delta

New rows for `Project Sources Security Checklist.md` (Stage 1) and
`Secrets Security Checklist.md` (Stage 2), each with a named test:

| checklist | control | test |
| --- | --- | --- |
| Project Sources | Containment refusal spans every environment and every project's added sources: a path that contains or sits inside a registered source, a vault path, a managed destination over a local source, or a second clone of a registered remote is refused; only an **equal** canonical directory may be shared | `source_overlap_refused_across_environments`; `same_directory_shared_by_two_projects` |
| Project Sources | Unlinking a reference never removes files, shared or not, managed clone or not; the folder stays for the user to delete | `unlink_keeps_folder_and_other_reference`; `unlink_last_managed_reference_keeps_clone` |
| Project Sources | A repository mutation on a shared checkout serialises on its Git lock and invalidates every referencing project's views; sync status stays per reference so two projects with different base branches never read each other's conclusion | `shared_repository_mutation_invalidates_every_referencing_project`; `shared_directory_different_bases_keep_separate_sync_status` |
| Project Sources | `project.*_source` cannot edit or remove an environment source through a project; the refusal names the environment | `project_verbs_refuse_environment_source_ids` |
| Project Sources | Config migration backs up, then writes nothing until every project is assigned; failure leaves the old file intact | `migration_failure_leaves_config_untouched` |
| Project Sources | Task attachment containment stays inside the bridge's attachment store (`local_attachments_dir`) and is never widened to a project's home or source directories; no containment check compares a key string | `attachment_containment_stays_in_attachment_store` |
| Project Sources | Within one project's effective sources a canonical directory appears once; unlinking never deletes a folder, including the last managed reference; repointing one reference never retargets a sibling's | `added_source_duplicate_within_project_refused`; `unlink_last_managed_reference_keeps_clone`; `repoint_reference_leaves_sibling_reference` |
| Project Sources | Recovery never registers a project from a persisted record; it resolves by key | `recovery_never_adds_project_from_record` |
| Project Sources | Names are one bounded line, no control characters, never a path | `rename_is_one_bounded_line` |
| Durable writes | Creation verbs are admitted by the frame's `operation_id`; a retry returns the recorded outcome; unknown blocks conflicting execution | `create_project_idempotent_by_operation_id`; `unknown_outcome_blocks_conflicting_retry` |
| Durable writes | `project.discard_unused` / `environment.discard_unused` refuse unless the target came from the named `creation_operation_id` and has no workspace, task or session (no projects, for an environment) | `discard_unused_refused_when_project_used`; `environment_discard_unused_refused_with_projects` |
| Durable writes | `writes.progress` and `writes.status.progress` never mark an outcome; confirmation comes only from a terminal record | `writes_progress_never_marks_outcome` |
| Tasks | Two projects in one environment number and list tasks independently; a task link to a sibling's workspace is refused | `sibling_projects_number_tasks_independently`; `link_workspace_of_sibling_project_refused` |
| Tasks | Every mutation on an archived project is refused on RPC, MCP and agent paths with one error code, except `project.restore` and (Stage 2) `project.delete` (1.9) | `archived_project_refuses_mutations_on_every_path`; `archived_project_accepts_restore_and_delete` |
| Tasks | `project.delete` removes only rows under the deleted project's key | `project_delete_removes_its_tasks_only` |
| Secrets (8) | Environment scope derives from the issued session's project's environment at read time; archived projects are not current | `environment_membership_revalidated_per_read`; `archived_project_agent_not_current_for_secrets` |
| Secrets (11) | Environment deletion revokes environment-scoped secrets before the registration is removed; project deletion leaves environment scope intact | `environment_delete_revokes_environment_scope` |
| Secrets (new) | A project-scoped secret is never released to a sibling's agent by list, attach, CLI release or audit, and never copied by a creation attempt | one negative test per mechanism |
| Secrets (19) | `secrets.environmentScope` capability, fixtures, manifest and version pins; an older client never sees the scope | `secrets_contract_includes_environment_scope` |
| Navigation (#523) | Kind glyphs are decorative (`aria-hidden`); device labels are part of each row's accessible text, since device identity is information; neither adds an interactive element; layout tests at 900 px unchanged | `pinnedInboxToolbarLayout.test.js`; `createWorkspaceWizardDom.test.js` (row accessible name includes the device) |

Rows touched but unchanged in meaning: Secrets 13–16 (values never in SPA
persistence or history; the environment sheet renders metadata only).
