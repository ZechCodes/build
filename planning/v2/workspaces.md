# Workspaces

## Purpose

A workspace is a durable, local working copy of a project's sources. It is the
place where agents, review surfaces, and user terminals operate. It outlives a
Finish action so the user can inspect or recover its files afterwards.

Creating and working in a workspace does not open an Issue or an active planning
flow. Those flows are outside the workspace lifecycle.

## Project sources

A project contains one or more ordered sources. A source is either a local
directory or a remote repository that Build clones when the project is created.
Local directories may be Git repositories or ordinary directories. Every source
has a stable source identity, a display name, and its source configuration.

Project edits are forward-looking. A workspace records a snapshot of its source
configuration when it is created. Adding, removing, or changing sources on the
project changes future workspace creation and does not rewrite, add to, remove,
or otherwise mutate an existing workspace.

Legacy projects whose project root is a Git checkout remain supported as a
single-source project. Their source is represented by `.` at the existing root;
Build does not move that checkout to another directory.

## Materializing a workspace

Build materializes each source independently beneath the workspace root.

| Source | Workspace materialization |
| --- | --- |
| Local Git directory | A separate Git checkout for that source. |
| Remote repository | Clone the remote, then use that clone as the source checkout. |
| Local non-Git directory | A copy of the directory and all of its files. |

Copying follows the selected isolation backend's defaults. For non-Git
directories, the worktree backend makes a normal independent copy. The
Rift backend delegates cloning to the installed Rift CLI and falls back to a
normal copy when Rift is unavailable or fails without leaving a destination.

Each source has independent state and recovery. After an interrupted creation,
Build reloads the workspace manifest and shows the incomplete workspace as
failed. Retry keeps completed sources and retries incomplete sources. If a
partial destination already exists, Build preserves it and asks the user to
inspect or move it aside before retrying; it never overwrites uncertain work.

## Source selection and Git views

The workspace menu presents directory tabs, one for each source. The selected
tab determines which directory the Files, Changes, and commit views display.
It does not select a terminal or alter another source's repository state.

A Git source exposes its own branch and tag selector. Choosing a branch or tag
runs a real Git checkout in that source directory. Git may carry compatible
working-tree edits to the selected ref. If the checkout would overwrite local
changes, needs a destructive merge, or cannot safely preserve the working tree,
Build does not change the ref and presents the error returned by Git. Build does
not force a checkout, discard local changes, or automatically stash them.

Choosing a tag checks out the tag and leaves that source in detached HEAD. The
Git view identifies the detached state clearly. Selecting another source leaves
the current source's checked-out ref and working tree unchanged.

An ordinary directory has Files but no branch/tag selector, commit history, or
Git changes view until it becomes a Git repository.

## Terminals

Terminals are scoped to the workspace root. They are not scoped to the currently
selected directory tab, a branch, or a tag.

- A new terminal starts in the workspace root, where every source is reachable.
- Selecting a different directory tab leaves every terminal session and its
  current working directory alone.
- Checking out another branch or tag changes files on disk but does not close,
  recreate, or retarget a terminal.
- If a user runs `cd api`, that terminal stays in `api` until the user or a
  command changes it.

## Finish and retention

Finish checks every Git source before completion. Each Git source must be pushed
to its configured remote; a source that cannot be pushed leaves Finish incomplete
and reports the source and Git failure. Non-Git sources are retained as they are.

Finish never deletes the workspace checkout or any workspace files. Retention is
part of the recovery contract: the user can return to the completed workspace,
inspect it, and continue manual work from the files that were used to finish.

## Boundaries

Workspaces provide source materialization, source-scoped file and Git views,
workspace-scoped terminals, and recovery. They do not replace the project source
configuration with changes made inside a workspace. They do not make a directory
tab into a terminal scope. They do not discard local edits to complete a ref
checkout or Finish.
