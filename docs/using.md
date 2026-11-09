# Using Build

How devices, environments, projects, workspaces and work isolation behave. For installing
Build, see the [README](../README.md#install).

## Devices

Every device you have paired shows up at once. The client opens a session to
each one, so the inbox and the projects rail list all of your machines' work
together. Where two machines hold projects with the same name, that name
carries the machine beside it (on the rows themselves, in the rail's project
headings and in the toolbar's project menu), so the two are never confused.
The device dropdown at the foot of the rail, beside **Account**, **filters**
that list (**All devices**, or one machine) and nothing else: it does not
move where anything runs, and a branch or task you have open stays open
whichever way the filter is set.

A machine that goes offline keeps its place: its rows stay in the rail, greyed
and marked offline, and its verbs come back the moment it reconnects. When no
machine can answer at all, Build waits for one rather than showing an empty
app.

Each device has its own settings page: the settings cog beside a device in the
dropdown opens it. That page is everything that belongs to that machine: its
projects folder, its projects list, **New environment…** (or **Add project…**
on older devices), agent modes, the default harness, and its **Work isolation**
choice (git worktrees or **Rift (copy-on-write)**, as below). The device must
be online to open its page.

**Settings** (the account page) holds what is not any one machine's: the
**Creation device** ("New projects and captures go to"), plus agent defaults,
appearance, notifications, downloads, and your devices and keys.

## Device project folders

On a device's settings page, use **Choose folder…** to browse that device's
filesystem and select the folder where its new projects should be kept.

The choice is saved on that device and survives bridge restarts. It applies to
new repositories and clones; existing repositories and task worktrees stay
where they are. `BRIDGE_PROJECTS_DIR` supplies the initial default when no
projects folder has been saved in the bridge configuration.

## Projects and workspaces

An environment holds source directories on one device. A project is one line
of work in that environment, with its own task board, agents and workspaces.
Picking the same environment for another line of work creates a separate
project with an empty board, numbered from #1; the existing project's tasks
stay on its board. Picking an existing project keeps using that board.

Sources can be local directories, Git repositories, or remote repositories
that Build clones. Each
source keeps its own files and Git state; a project may also include ordinary
directories that have no Git repository.

When Build creates a workspace, it materializes each source separately. A
non-Git source is copied using the selected backend: a regular copy with the
worktree backend, or the installed Rift CLI with the Rift backend, falling
back to a regular copy when Rift is unavailable or cannot create the copy safely.
Git sources are checked out independently.
An older project whose root is itself a Git checkout remains a single-directory
project represented by `.` and is never moved.

Directory tabs follow the workspace menu and collapse into a directory menu
on phones. Selecting a directory controls the Files, Changes, and commit views.
All changes includes unpushed commits and uncommitted edits, using locally known
remote history as its baseline. A searchable selector above the commit list
separates branches and tags and identifies remote branches and available pulls.
Switching a ref performs a real checkout in that source. Compatible
uncommitted edits remain in place; if a checkout would overwrite or otherwise
lose local changes, Build leaves the source untouched and reports Git's reason.
Selecting a tag results in a detached HEAD and the interface says so.

The workspace wizard and project settings select existing folders. The legacy
Add project… picker can also create a new folder while adding project sources.
From a workspace directory, Initialize Git offers the workspace copy, the
original project source, or both. Initializing the source enables Git for future
workspaces; initializing only the copy leaves the original source unchanged.
When they are separate folders, initializing both creates independent
repositories and preserves the files in each. If the workspace and source are
the same folder, Git is initialized there once.

Terminals belong to the workspace, not to the selected source or ref. A new
terminal starts at the workspace root and remains open when the selected source
or its ref changes. A terminal that has changed directory stays where its shell
put it; selecting a source does not change its working directory.

Finishing a workspace verifies that every Git source is pushed to its remote,
then retains the workspace checkout and all of its files. Build never deletes a
workspace as part of Finish. It retains per-source recovery state so an
interrupted operation can resume safely. Later edits to a project's source list
affect new workspaces only: an existing workspace continues from its recorded
source configuration.

For the complete workspace behavior, see
[`planning/v2/workspaces.md`](../planning/v2/workspaces.md).

### Create a workspace

On a device with environments support, **New workspace** in the inbox opens
the **Where?** picker. It shows Recent targets, then **Projects** (uses the
existing task board), then **Environments** (creates a new task board). Search
by name; when more than one device is listed, rows carry device labels. The
target you choose determines where the workspace runs. **New workspace in
this project** starts with that project already selected.

The naming page asks **What are you working on?** A name is required for a new
line of work in an environment and optional for a workspace in an existing
project. It previews the branch name, offers isolation, and lists the
directories the workspace will contain. **Create workspace** starts one
attempt: a workspace in the selected project, a new project and its first
workspace in the selected environment, or a new environment, project and
workspace together.

### Create a new environment

Choose **New environment…** in the picker. On **New environment**, choose the
device if more than one is offered, then **Add folder** to select an existing
folder, or **Add Git remote** and enter its **Git remote URL**. You can add
several sources, give each a **Folder label**, set **Base branch (optional)**,
and change **Environment name (optional)** and the environment's **Isolation**.
A Git URL is cloned on that device when you create the workspace.

**Next** opens the naming page. Until **Create workspace**, these are only
drafts: browsing, naming and going back do not register an environment or
project, clone a repository or create a workspace. Create workspace submits
one attempt for all three, with **Creating environment**, **Creating project**
and **Creating workspace** shown in order.

The **Environments** settings page also offers **New environment…**, **New
project in E…** and **New project with extra folders in E…**, where E is the
environment's name. Each opens the same wizard and says **Creates a workspace**;
the extra-folders shortcut opens **Advanced** for you.

### If creation fails

The progress screen shows Creating…, Done, Failed or **Checking…**. Checking
means Build is finding out what happened; it does not offer a retry that could
create duplicate work. If the project was created but its workspace failed,
the screen lists what landed, including the environment when it was created
by this attempt, and offers **Try the workspace again**, **Keep it** and
**Discard this project**. Retrying creates only the workspace in that project;
it does not create another environment or board. Discard only works for the
project from that attempt while it has no workspace, task or agent session.

A refusal before anything is written returns to the relevant folder or naming
page with the draft intact. If a failure reaches progress with nothing
created, **Back to the wizard** returns to that draft. A failed Git clone for
a new environment leaves no environment registered.

If the environment was created but the project was not, progress says **no
projects yet** and offers **Resume**, **Keep the environment** and **Discard
the environment**. Resume starts a new attempt for a project and workspace in
that environment. Discard removes only the empty environment created by that
attempt and keeps its source folders; it is refused once projects belong to
it. A kept empty environment remains labelled **no projects yet** in the
picker and Environments settings, with **Resume…** to continue later.

Closing progress leaves the attempt running; reopening the wizard can recover
its saved progress, including after a page reload. Done offers **Open workspace**.

Devices without environments support use the legacy workspace form. Devices
without new-environment creation use **Add project…** in device settings;
the legacy sheet registers sources and an initial project. New environment…
in device settings starts with that device selected; the workspace picker
uses the selected target's device. Items from the capture compose box use the
**Creation device** default on Settings.

### Extra folders for one project

When starting a new line of work in an existing or new environment, expand
**Advanced** on the naming page: **Also add folders to this project only**.
Use **Add folder** or **Add Git remote** there. These sources belong to the
new project, alongside the environment's folders; sibling projects do not
gain them. Advanced is absent when adding a workspace to an existing project.

For an existing project, open **Project settings** → **Sources**. **From E**
lists the environment's folders read-only. **Added to this project** holds
editable cards with **Save** and **Remove**; **Add folder…** and **Add Git
remote…** add sources directly to this project without opening the workspace
wizard. Changes to this source list affect new workspaces; existing workspaces
keep their recorded directories.

### Shared folders

Two environments or projects can use the same folder. In Advanced, a known
reference is labelled **shared with E** or **shared with P**, using the
environment's or project's name; otherwise it says **new folder**. These
labels describe known references; Create workspace still checks the actual
folders. A folder already included in this project's environment or added
folders is refused, as is a folder inside or containing any registered source.
Use the existing folder to share a repository; cloning a second copy of an
already registered remote is refused.

Sharing points both projects at the same original files and repository. Edits
in that original folder are visible to both projects. Each workspace still
gets its own worktree or copy; the projects keep separate boards, agents and
workspace lists. Folder labels, paths, base branches and sync choices belong
to each reference: changing one does not retarget the other. Removing a
reference leaves the files on disk, even for the last reference to a
Build-managed clone.

A remote change, fetch or other Git operation on the shared repository affects
that repository for both projects. Build serializes repository operations and
refreshes every referencing project's views. Sync status stays separate for
each project's source, so projects using different
base branches or sync choices keep their own results.

### Environment settings and names

Open **Settings** → **Environments**, then **Settings…** on the environment's
card to open **Environment settings**. Its **General** section has
**Environment name** and **Save name**; **Home path** is read-only. For a
project, open **Project settings** → **General**, edit **Project name** and
choose **Save name**. The project's **In environment E** link also opens
that environment's settings, where E is its name.

Both names are display names: renaming changes no folders, ids, task keys or
branches. An environment rename does not rename its projects. A name must be
non-empty after trimming, fit on one line without control characters, and use
at most 200 bytes. A refusal reads **A name must be one bounded line (at most
200 bytes).**

The environment sheet's **Sources** section edits the folders inherited by
all its projects. Cards have **Label**, **Folder**, and, for Git sources,
**Base branch** and **Remote (origin)**, with **Save** and **Remove** where
allowed. The first source's Folder is read-only and it cannot be removed.
**Add folder…** selects an existing folder; **Add Git remote…** clones another
repository. Shared cards say **also used by P**, with the project's name.
The shared-folder rules above still apply, and removing a reference keeps the
files. Membership changes affect new workspaces; existing workspaces keep
their recorded directories.

This differs from **Added to this project** in Project settings, which edits
only that project's extra folders. **From E** remains read-only there;
**Edit in E** opens the environment sheet to edit the inherited folders.
These settings actions edit existing sources directly and create no workspace.

Under **Isolation** on the environment sheet, choose **Git worktree**,
**Rift (copy-on-write)** or **Inherit device setting**. Projects without their
own override inherit that choice; existing checkouts keep their isolation.

### Archive a project

Open **Project settings** → **Archive** → **Archive project**. Archiving
keeps the project and its history, and refuses while work still needs to be
settled. It does not stop work for you. The refusal is shown exactly as the
bridge returns it: **Cannot archive P:** followed by each blocker, separated
by semicolons. P is the project's name; workspace and agent names, task
numbers and terminal ids identify what needs attention:

- Unfinished or provisioning workspaces: **finish or delete workspace W**.
  A pending workspace creation says **wait for workspace W creation to finish**.
- Running, starting or queued agents: **stop worker X** or **stop coordinator X**.
  Queued, in-flight or settling wakes say **wait for pending wake for O**,
  where O is the owning conversation's id.
- Terminals: **wait for terminal creation to finish**, or **close the open
  terminal in this project**. A removed or replaced terminal or agent still
  shutting down says **wait for the closed terminal to exit (T)** or
  **wait for the stopped agent to exit (A)**, with its terminal or agent id.
- Tasks needing attention or blocked on other tasks: **answer #12** or
  **resolve waiting-on for #12**. Assignment and dependency work says
  **settle pending delivery for #12** or **settle pending dependency
  reconciliation for #12**.
- Reviews and their handoffs: **give a verdict on #12**, **settle pending
  verdict delivery for #12**, **settle pending revision dispatch for #12**,
  or **settle revision delivery notice for #12**. Other queued task notices
  say **settle pending task notice for #12**.
- Filesystem work still running: **wait for filesystem operations to finish**.

Clear the named blockers and press **Archive project** again. The button is
disabled only while one request is pending; a refusal leaves it available for
a new attempt, because a blocker can clear without the client knowing.

An archived project leaves the inbox, the workspace wizard, the switcher's
ordinary list and Recent targets, and the workspace selector. Find it by
searching the switcher, where it is tagged **Archived**, or in **Environments**
settings under its environment's collapsed **Archived** group. Direct links
to the project, its old tasks and reviews still open read-only. Their banner
says **P is archived. Restore it to make changes.** and offers **Restore**.

The bridge allows Restore and its existing `project.delete` on an archived
project; other changes from the UI, MCP and agents are refused, and its agents
are not started. The bridge refusal is **P is archived; restore it first**,
with error code `project_archived`. Reading its history still works.
Environment source maintenance can still update the inherited source list;
it does not change the archived project's finished workspace directories.

### Restore a project

In **Environments** settings, expand **Archived** under the environment and
choose **Restore** beside the project. The banner on a directly opened
project, task or review offers the same action; Project settings also offers
Restore. The project returns to active lists with the same identity, task
board, reviews, comments, runs, plans and finished workspaces. Changes are
available again and retained background work becomes eligible to resume.

### Current MVP limitations

- Environment secrets are not available yet; existing Agent and Project
  secret scopes stay separate.
- Environment deletion and deleting a project together with its tasks are
  not available yet. The existing Delete project behavior is described below.
- MCP tools for managing environments and identity badges are not available yet.

### Delete a project

**Delete project** is available in **Project settings** for active projects,
including ones under an environment. Restore an archived project before
deleting it from this sheet. It requires confirmation and is refused
while creation is pending, another filesystem operation is running, or a
workspace is locked or provisioning. Pending shell launches and in-flight
agent messages in the project's source or checkout folders also block deletion.

Deletion removes the project and its workspaces from Build. Files and unsaved
changes in Build-managed workspaces, Build-created agent run checkouts and the
project conversation's scratch folder are permanently removed. Associated
agent runs and their conversations, legacy plans and their saved documents,
and captures routed to the project are deleted. Build revokes the project's
secret grants and agent access, cancels pending task deliveries, and clears
its retained inbox activity history. Original project folders and external
checkouts are kept.

Task-board records stay stored with their identities preserved; these are
different from the legacy plans deleted above. Keeping those records does not
keep the deleted project or its board available. **Discard this project** is
the separate, narrower action offered immediately after a failed creation
attempt.

## Environment config migration and rollback

The bridge privately migrates each configured project into its own environment,
including projects whose folders are temporarily unavailable. Existing project
ids and task keys stay the same; tasks, workspaces, runs and scratch folders do
not move. M1 exposes the environments and workspace picker described above.

Before its first migration write, the bridge saves the exact original config as
`config.json.pre-environments`, beside `config.json`. That snapshot is written
once and never replaced. Each successful config replacement also retains the
previous current config in `config.json.previous` (one generation). Migration
errors stop loading and leave the current config unchanged.

**Downgrade boundary:** the bridge store is now schema 26. A binary that
supports an earlier schema refuses to open it before applying database pragmas
or schema changes, and the bridge fails to start. Going back across schema 26
is unsupported, even if no sibling project has been created. Restoring only an
older config does not make the database compatible. Recover by fixing or
updating the newer bridge (roll forward).

The config still contains legacy project fields. The frozen legacy-loader
regression checks only those config bytes: it can load the original projects,
but collapses siblings sharing a home into the first project at that path;
the later entry replaces its source settings, isolation and source counter.
It ignores the sibling's separate task key, so the sibling's tasks, scratch
and runs are not guaranteed to stay bound or reachable. That config-loader
result is not a promise that the old binary can open the current store.

Restoring `config.json.pre-environments` deliberately abandons registrations
made since migration, including new project names, keys and source references.
Their task and scratch records remain stored under keys no registered project
claims; restoring that old snapshot does not recover them. For a bad write on
the newer bridge, use the previous-current snapshot instead. Any authorized
binary rollback within a compatible store schema uses
`scripts/buildctl --env <env> bridge rollback` with the environment named;
stop the bridge before deliberately restoring a config.

## Work isolation

Build uses Git worktrees by default. For copy-on-write checkouts, install the
[Rift CLI](https://github.com/anomalyco/rift#install) on the machine running the
bridge and make `rift` available on the bridge's `PATH`. Select **Rift
(copy-on-write)** in Work isolation settings, either as that device's default
or as a project override. The choice applies to new checkouts.

A project without its own override inherits its environment's isolation
setting, then the device default if the environment has no setting. In
Project settings, clearing the override returns to the environment's setting
without changing the environment or any sibling project. The inheritance
option names the environment and the isolation it supplies. The wizard can
also override isolation for the workspace being created; existing checkouts
keep their isolation.

Rift owns filesystem cloning and snapshot creation. Build requests full copies
to retain ignored build caches, skips Rift hooks, and checks out the task's
branch for Git sources. Workspace completion verifies pushes and retains files
with either backend. Build does not install or update Rift for you.

The first Rift task initializes its source project. On Btrfs, Rift may convert
the source directory into a subvolume; on other supported filesystems it
registers the directory in place. Git sources must have their own `.git`
directory; ordinary directories need no Git metadata. The checkouts folder
must be outside the source. Filesystem support and
initialization errors are reported by Rift when creating the checkout.

Build keeps a private Rift registry under each project's checkouts folder at
`.rift/registry.sqlite`, so its garbage collection does not touch workspaces
registered by other Rift users. A project already registered in a different
Rift registry cannot be initialized in Build's registry; Build leaves its
marker untouched and reports the conflict. Keep the Rift CLI installed while
Build has Rift checkouts to manage.
