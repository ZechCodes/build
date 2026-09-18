# Workspaces

## Purpose

A workspace is a durable, local working copy of a project's sources. It is the
place where agents, review surfaces, and user terminals operate. It lasts until
its work is somewhere else — see Finish and retention.

Creating and working in a workspace does not open an Issue or an active planning
flow. Those flows are outside the workspace lifecycle.

## Project sources

A project contains one or more ordered sources. A source is either a local
directory or a remote repository that Build clones when the project is created.
Local directories may be Git repositories or ordinary directories. Every source
has a stable source identity, a display name, and its source configuration. An
identity is never reissued: the project mints from a count of its own, held past
every id it has ever carried, so a removed source's id is not handed to another
folder while a workspace cut from it still names it.

Sources are added and removed after a project is opened. `project.add_source`
appends one — a path on the device, or a remote Build clones into the project's
own sources folder — the same way the project was opened over the sources it
has; `project.remove_source` takes one off. A project always has at least one.

A project edit is forward-looking. A workspace records a snapshot of its source
configuration when it is created, and a project edit changes what the next
workspace is cut from: it does not rewrite, add to, or remove a directory in a
workspace that already exists. A workspace's own directories are changed by
`workspace.add_directory` and `workspace.remove_directory`, which name the
workspace they act on.

`workspace.add_directory` materializes one more directory into a workspace root
that is already there — a project source not in it yet, a path on the device, or
a remote to clone — using the same per-source work creation does: a Git source
becomes a checkout on a branch of the workspace's own cut from that source's
base branch, anything else is copied. The directory is provisioning until it
lands. A path is refused when it is, contains, or is contained by another
project's source: cutting a checkout there would put a branch and a worktree
registration in a repository this workspace was never cut from, which is what
`project.add_source` refuses in the same words.

`workspace.remove_directory` hands its checkout back to the repository it was
cut from and takes the folder, closing the agents and terminals standing in that
directory and leaving the ones whose cwd is elsewhere. It applies what removing
a whole workspace applies, narrowed to one folder: it refuses an adopted
workspace, a directory that resolves outside the workspace root, a directory
that is the root itself, one that holds a registered source repository, and a
removal while an agent is working at the workspace root — that session is keyed
at the root, so it is working in every directory of the workspace at once.

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

Done is offered on a workspace only once its work is somewhere other than that
workspace: every Git source clean, with no commit its push destination does not
already have, no ordinary directory left in it, and no agent turn in flight at
the workspace root. A row says which of those is in the way — `dirty`,
`unpushed`, `plain_directory`, `agent_working`, or `unknown` while nothing has
managed to read the repositories — so the control can say it too.

An ordinary directory cannot be measured, which is why it blocks. Build has no
published baseline to call its files unchanged against and no remote holds a
copy of them, so Done — which removes the workspace — cannot say that work is
anywhere else. `workspace.remove_directory` is the way out: take the folder off
the workspace, and what is left is repositories Done can measure.

Done removes the workspace. It closes every agent and terminal standing in it,
hands each checkout back to the repository it was cut from, and walks the root
away; the live record goes with the files, so `workspace.list` stops naming it
and the conversation that stood in it ends. There is no second, gentler Finish:
`workspace.finish` is this whether or not it is asked for a clean one, and the
old spellings (`run.finish`, `branch.finish`, `worktree.finish`) resolve to the
same workspace and the same rule.

Done never pushes. Publishing is the user's, which is what the eligibility above
is measuring: by the time Done appears, every commit the workspace holds is
already in the remote it pushes to. Recovery is pulling that remote.

What survives is the record of what was finished: which workspace, when, and the
commit each source ended on. It is kept beside the registry rather than inside
the root, so removing the workspace cannot take it, and it is what the archive
lists. There is nothing behind it to open.

A checkout Build did not make is not Build's to remove, so Done refuses an
adopted checkout the way Delete does.

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

`project.list` carries that owner too — `entity_id` and `run_id`, both `null` on
a project nobody has talked to yet — read from the same lookup
`project.ensure_conversation` makes before it mints. It is the only read of the
owner there is, and it exists because a surface that shows a project's agent has
to be able to learn the project has none without minting one to find out.

The owner's agents work in a durable scratch directory Build owns, never in the
project's checkout — the project is the template workspaces are cut from, and
talking about it must not change it. The directory sits under Build's own state
directory, keyed by the project's canonical path, and is never wiped: a `proj-N`
id is not durable across boots, and the conversation is. Abandoning the owner
ends the conversation and lets go of the directory rather than removing it, the
way a run standing in the project's own repository does, so the next
`project.ensure_conversation` hands the new owner back everything the last one
wrote.

A project's conversation owner is not one of that project's workspaces. It is a
run standing in Build's scratch directory, and legacy adoption — which imports
every run with no base branch whose root no manifest names — used to import it as
one. `workspace.list`, the feed and the project agent's own `list_workspaces`
then carried a workspace named after the project, rooted in the scratch
directory, and the agent found itself in its own list, read its own roster and
sent itself work. Adoption skips project conversation owners, and
`workspace_conversation_owner` never answers one, so no workspace resolves to the
project's own roster however it is addressed.

What a project agent starts on — its harness, model and reasoning effort — is a
DEVICE setting, held by the bridge beside `default_harness` and carried on
`settings.get` / `settings.set` as `project_agent: { provider?, model?, effort? }`
(wire spec, step 2.7). It is one choice for every project, not one per project:
it is a preference about a kind of agent. A project agent talks ABOUT a project
rather than working in a checkout, which is why the harness for that job is
asked for it alone — and it is asked of the machine that will run the agent,
which already knows the answer, rather than of each browser at first use.

Every absent word names a default that already stands: no provider is the
device's default harness, no model or effort is that harness's own. So a device
that has chosen nothing changes nothing.

Every mint of an agent on a project's conversation owner spends that setting:
the one `project.ensure_conversation` makes, the one a message to a project
nobody is on forces, and an `agent.add` that names no choice. It is read AT the
mint, so moving the setting moves what the next project agent opens on and
leaves the one already running alone. The flattened `provider`/`model`/`effort`
of `project.ensure_conversation` are the API that verb has always had and still
win where a client sends them; the SPA sends none.

The choice is made on the DEVICE settings page, in a Project agent panel beside
the fallback agent (`core/projectAgentSetting.js`): three selects that narrow
into each other — the harness, that harness's models, that model's reasoning
levels — painted from `settings.get` and `models.list`, saved on every change,
and repainted from the set's own answer. The rail standing on a project's
conversation reads the same setting to seed a NEW project agent, falling back to
the device's default harness where it names none; a model belongs to its
harness, so pressing another harness card leads with that harness's own
defaults. Nothing about a project agent is kept in a browser.

The project's agent is reachable from every workspace in the project — that is
what makes it the project's rather than a workspace's — so every workspace's
rail carries it too: one bubble wearing the project's initial above a line, with
that workspace's own agents below it. It is there before the project has a
conversation at all, reading as an agent waiting to be started, and the press is
what mints the owner: a rail that minted one to paint a bubble would give every
workspace page a project agent, a scratch directory and a run nobody asked for.
Pressing it stands the same rail on the project's conversation, in the same
panel, with the page still on the workspace; pressing a bubble below the line
stands it back. The rail reads the conversation it is not standing on beside its
own, so both halves of the strip carry their unread and their working.

A message sent from there says where it was sent from. The rail leads the
message's `viewing_context` with `{ kind: "workspace", workspace_id, name }`
(wire spec, step 2.8) — the id it was mounted with and the name its own read
already answered — ahead of whatever the reader attached, and the composer's
tray shows it as a chip they cannot remove: standing somewhere is not an
attachment. It is stamped by the rail that got here from a workspace and by
nothing else, so the project's own page, which stands in no workspace, sends
none, and a workspace's own agents, who are already in one, are told nothing.
The project agent then reads one line in its delivery envelope — `The user sent
this from workspace "<name>" (<workspace_id>).` — and the same fact on the
message's own line in a cold catch-up packet, so it knows what "this workspace"
means whether it was running or has just been started.

## The project agent

A project's agents reach Build through a surface of their own. It is the same
one MCP server every session gets — one `mcpServers.build` entry, one
`build-bridge mcp --task <id>` process — and the id the session was opened with
decides which tools are on it, the way a `router-` id decides the router's. A
project agent's id starts with `project-`, and the prefix is minted from the
owner: an agent of a project's conversation owner is a project agent, whoever
asked for it.

The surface reads and changes the project, and is ordinary about its
conversation. Every tool is a thin wrapper over the verb the client calls — the
same code path, the same refusals, the same record afterwards:

- `list_workspaces` — every workspace of the project, through the same code path
  `workspace.list` answers, so the agent and the client see one list.
- `list_workspace_agents {workspace_id}` — the agents on one workspace's
  conversation, through `agent.list` on that workspace's conversation owner.
- `create_workspace {name, isolation?}` — through `workspace.create`, with the
  project supplied by the binding.
- `add_workspace_agent {workspace_id, harness?, model?, effort?}` — through
  `agent.add` on the workspace's conversation owner, minting that owner with
  `workspace.ensure_conversation` when the workspace has none yet.
- `remove_workspace_agent {workspace_id, agent_id}` — through `agent.remove`.
- `message_workspace_agent {workspace_id, agent_id?, body}` — through
  `thread.post` on that agent's conversation; naming no agent is the
  workspace's primary one.
- `delete_workspace {workspace_id}` — through `workspace.delete`, refusals and
  all.
- `add_project_source {path? | remote?, name?, base_branch?}` and
  `remove_project_source {source_id}` — through `project.add_source` and
  `project.remove_source`, with the project supplied by the binding.
- `add_workspace_directory {workspace_id, source_id? | path? | remote?, name?}`
  and `remove_workspace_directory {workspace_id, directory_id}` — through
  `workspace.add_directory` and `workspace.remove_directory`.
- `post_thread_message`, `search_conversation`, `set_topic` — what every agent
  with a conversation has.

Which project is read or written comes from the owner's project binding and
never from a tool argument, so the scope is fixed when the agent is created: a
workspace in another project is refused by name, and a call that carries a
project id is parsed as though it had not. The gate is on the socket as well as
in the tool list a session is shown, so a harness writing its own frames reaches
no further than a harness that reads the list.

A message to a workspace agent goes in with the user's role — that is the side
of the conversation an instruction arrives on whoever wrote it — and wears the
project agent as its `from_agent`, so the agent reading it knows a machine sent
it. The operation the post creates remembers the requester: which project agent
asked, the owner it belongs to, and its own conversation, which is where the
answer is owed. `note_user_message` is skipped for it, because one agent handing
work to another is the work happening and must not move the inbox anchor under
the reader. For the same reason it crosses no dismissal line: a row the human
cleared stays cleared until somebody speaks to THEM, so the line is the newest
message no other agent signed. The store keeps that line as a column of its
own, so it reads the same after a restart that loads a conversation as a tail
of hand-offs.

The answer comes back on its own. When the workspace agent ends that turn — a
`post_thread_message` with status Complete or Blocked — its terminal message is
posted into the project agent's conversation and delivered there the way any
turn is: the user's role again, wearing the workspace agent as its `from_agent`,
with the status on the first line and the agent's own words under it. The coding
agent needs no tool for this and is never told it was summoned by a machine
rather than by the user; the project agent reads the reply as a message, not as
a tool result.

Only a terminal message travels. Working and Waiting keep the turn open, so a
progress note or a question does not interrupt the project agent mid-turn, and
the answer it is waiting for is still owed. And a forwarded answer is never
itself forwarded: it records no requester of its own, so the turn it starts owes
nobody a reply and nothing can bounce between two agents. The requester is
settled as it is read — one message handed over, one answer handed back — and a
forwarded answer skips `note_user_message`, and crosses no dismissal line, for
the same reason the message that asked for it does. What the workspace agent
said in its OWN conversation is not a hand-off and still calls the human: it
stopped and said so.

The agent runs in the project's durable scratch directory, holds no checkout,
and receives the same delivery envelope, catch-up packet and topic handling as
any other agent. What it does not receive is the coding prompt: that one is
about phases, a plan and a diff, and this agent has none of them. It is told
which project it is the agent of, why the project's own files are not its to
change, and what its tools are — including that a removal takes whatever is in
what it removed and is not committed and pushed anywhere else. Its messages carry a status and no phase,
so a terminal message ends the turn and reports no lifecycle outcome.

## Boundaries

Workspaces provide source materialization, source-scoped file and Git views,
workspace-scoped terminals, and recovery. They do not replace the project source
configuration with changes made inside a workspace. They do not make a directory
tab into a terminal scope. They do not discard local edits to complete a ref
checkout or Finish.
