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
projects folder, its projects list, **Add project**, agent modes, the default
harness, and its **Work isolation** choice (git worktrees or **Rift
(copy-on-write)**, as below). The device must be online to open its page.

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

The directory picker can create a new folder while adding project sources.
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

The next page asks **What are you working on?** A name is required for a new
line of work in an environment and optional for a workspace in an existing
project. It previews the branch name, offers isolation, and lists the
directories the workspace will contain. **Create workspace** starts one
attempt: either a workspace in the selected project, or a new project and
its first workspace in the selected environment.

The progress screen shows Creating…, Done, Failed or **Checking…**. Checking
means Build is finding out what happened; it does not offer a retry that could
create duplicate work. If the project was created but its workspace failed,
the screen offers **Try the workspace again**, **Keep it** and **Discard this
project**. Discard only works for the project from that attempt while it has
no workspace, task or agent session. If nothing was created, **Back to the
wizard** lets you correct the draft. Closing progress leaves the attempt
running; reopening the wizard can recover its saved progress, including after
a page reload. Done offers **Open workspace**.

Devices without environments support use the legacy workspace form. To set up
a new environment today, use **Add project** in device settings: the legacy
sheet still registers the sources and an initial project. The wizard has no
**New environment…** page yet. Add project starts with that device selected;
the workspace picker uses the selected target's device. Items from the capture
compose box use the **Creation device** default on Settings.

The current M1 limitations are:

- No extra-folder selection in the wizard; it uses the target's sources.
- No archive or rename, and no new project lifecycle flow in environment
  settings. Every new line of work stays in the inbox until archive arrives,
  unless you delete the project.
- No environment secrets.

**Delete project** remains available in **Project settings** for any project,
including one under an environment. It requires confirmation and is refused
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
