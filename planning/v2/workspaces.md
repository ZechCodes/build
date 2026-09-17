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

A project's own checkout is a source, never a place to work. Build does not adopt
it as a workspace-like checkout, shows no row for it, and refuses `run.adopt` of
it — naming workspaces as the way to work. It is still read: files, diffs, branch
listings and terminals resolve to it. No verb that removes a run's checkout may
remove it, whatever a record written before this says.

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

Directory tabs follow the workspace menu, collapsing into one menu on phones. The selected
tab determines which directory the Files, Changes, and commit views display.
It does not select a terminal or alter another source's repository state.

A Git source exposes a searchable selector above its commit list, with separate
Branches and Tags tabs, remote branches, and indicators for available pulls.
The indicators reflect locally fetched remote history. Choosing a branch or tag
runs a real Git checkout in that source directory. Git may carry compatible
working-tree edits to the selected ref. If the checkout would overwrite local
changes, needs a destructive merge, or cannot safely preserve the working tree,
Build does not change the ref and presents the error returned by Git. Build does
not force a checkout, discard local changes, or automatically stash them.

All changes combines unpushed commits with staged, unstaged, and untracked
changes. Its baseline is the common ancestor with the branch's push target;
without that target, it uses the nearest known published ancestor. A repository
with no known published history shows its full contents. Reading this view
does not fetch or modify Git state.

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

## The project page

A project has a surface of its own: `#/project/<project_id>`, and
`#/device/<device_id>/project/<project_id>` where the machine is named. It is
about the project, never about the project's checkout — that checkout is what
workspaces are cut from and has no surface at all — so every URL that used to
open it lands here instead.

The main pane is the project's workspaces: the same rows the landing rail groups
into that project's block, in the same order, each opening its own workspace.
Beside them the page says what each workspace is standing on and how its checkout
is doing. A project nobody has cut a workspace in yet is named and empty, which
is the ordinary first state of a project rather than a missing one; the empty
state points at the + that makes the first workspace.

The toolbar names the project and carries the page's two verbs: the + and the
cog that opens project settings. The rail is the project's own agent, mounted on
the owner `project.ensure_conversation` answers with.

A project belongs to one machine, and every machine mints a `proj-1`, so a
project URL that names no device is a question: it parks on the same resolve hop
every other work URL parks on, and the feed's projects answer it — which is what
lets a project with no rows of any kind still open.

## Project conversations

A project is a conversation owner in its own right, the way a workspace is:
`project.ensure_conversation` answers the owner a project already has, or mints
one, and answers `{project_id, entity_id, run_id}` either way. A second call
answers the first one's owner, and the owner survives a bridge restart.

The owner's agents work in a durable scratch directory Build owns, never in the
project's checkout — the project is the template workspaces are cut from, and
talking about it must not change it. The directory sits under Build's own state
directory, keyed by the project's canonical path, and is never wiped: a `proj-N`
id is not durable across boots, and the conversation is.

## The project agent

A project's agents reach Build through a surface of their own. It is the same
one MCP server every session gets — one `mcpServers.build` entry, one
`build-bridge mcp --task <id>` process — and the id the session was opened with
decides which tools are on it, the way a `router-` id decides the router's. A
project agent's id starts with `project-`, and the prefix is minted from the
owner: an agent of a project's conversation owner is a project agent, whoever
asked for it.

The surface is read-only about the project and ordinary about its conversation:

- `list_workspaces` — every workspace of the project, through the same code path
  `workspace.list` answers, so the agent and the client see one list.
- `list_workspace_agents {workspace_id}` — the agents on one workspace's
  conversation, through `agent.list` on that workspace's conversation owner.
- `post_thread_message`, `search_conversation`, `set_topic` — what every agent
  with a conversation has.

Which project is read comes from the owner's project binding and never from a
tool argument, so the scope is fixed when the agent is created: a workspace in
another project is refused by name. The gate is on the socket as well as in the
tool list a session is shown, so a harness writing its own frames reaches no
further than a harness that reads the list.

The agent runs in the project's durable scratch directory, holds no checkout,
and receives the same delivery envelope, catch-up packet and topic handling as
any other agent. What it does not receive is the coding prompt: that one is
about phases, a plan and a diff, and this agent has none of them. It is told
which project it is the agent of, why the project's own files are not its to
change, and what its five tools are. Its messages carry a status and no phase,
so a terminal message ends the turn and reports no lifecycle outcome.

## Boundaries

Workspaces provide source materialization, source-scoped file and Git views,
workspace-scoped terminals, and recovery. They do not replace the project source
configuration with changes made inside a workspace. They do not make a directory
tab into a terminal scope. They do not discard local edits to complete a ref
checkout or Finish.
