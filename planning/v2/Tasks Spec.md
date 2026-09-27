# Tasks

## Purpose

A task tracker, one per project, that is where the user and the agents working
that project agree on what is being done. It reads like GitHub issues — a title,
a markdown body, a state, labels, an assignee, comments and a timeline — with a
kanban view over status columns.

What makes it Build's rather than a copy of GitHub's: **assigning a task is
dispatching it**. A task handed to an agent arrives in that agent's
conversation and the agent starts on it. There is no second step where somebody
turns a task into work.

Agents are first-class here. An agent of a project can list, read, file, comment
on, move, close, link and assign tasks in its own project, and can assign to
another agent of that project, to the project's own agent, to the user, or to a
workspace and agent it asks Build to create. It cannot reach another project's
tasks however a call is spelled.

This document is phase 1: the bridge. The SPA phase follows from
[What the SPA phase builds](#what-the-spa-phase-builds).

## Not the plan flow

Build already has a table called `tasks` and a family of `task.*` / `plan.*`
verbs. Those are the retired **plan** flow — a goal, stage documents, approvals
and implementation runs. This is a different thing that happens to share the
English word.

Nothing here extends that flow. The tracker gets its own tables
(`tracker_tasks`, `tracker_comments`, `tracker_events`) and its own verb family
(`tasks.*`, which cannot collide with the singular `task.*`). Everywhere else
in this document, "plan" means the legacy flow and "task" means the tracker's.

## The record

### Task

| Field | Wire | Notes |
| --- | --- | --- |
| `id` | `"task-01K5Z…"` | ULID-style, the same mint agent ids use: 48 bits of milliseconds then 80 bits of randomness, Crockford base32, under an `task-` prefix. Time-ordered, so ids sort the way the tasks were filed. |
| `project_id` | `"proj-1"` | The project the task belongs to. |
| `number` | `12` | Per-project, sequential from 1, for `#12`. Never reused. |
| `title` | string | Trimmed, non-empty, at most 200 characters. |
| `body` | markdown string | May be empty. At most 32 000 bytes, the bound a thread message already carries. |
| `state` | `"open"` \| `"closed"` | |
| `status` | `"backlog"` | The kanban column, as a **slug string** — see [Columns](#columns). |
| `labels` | `["bug", …]` | Free strings, deduped, trimmed, at most 20, each at most 40 characters. |
| `priority` | `"none"` \| `"low"` \| `"medium"` \| `"high"` \| `"urgent"` | `none` is the default and is a value, not an absence. |
| `assignee` | `null` \| assignee | See [Actors](#actors). |
| `links` | object | See [Links](#links). |
| `trackers` | `["agent-01K5Z…"]` | The agents watching this task — see [Tracking](#tracking). Ordered by when each started tracking, deduped, at most 50. Always present; a task nobody watches answers `[]`. |
| `created_by` | actor | Who filed it. |
| `created_at`, `updated_at` | RFC 3339 UTC | |
| `closed_at` | RFC 3339 UTC \| `null` | Set when `state` becomes `closed`, cleared on reopen. |
| `done_at` | RFC 3339 UTC, absent | When the task last moved into Done, while it is there; absent anywhere else. Stamped by the move (or by filing it in Done) and cleared when it leaves. A task written before the field existed gets it from its timeline's latest move into Done the next time `tasks.list` or `tasks.get` reads it, and keeps it. Announced as `tasks.doneSinceLeft`. |

### Comment

| Field | Wire | Notes |
| --- | --- | --- |
| `id` | `"tc-01K5Z…"` | Same mint, `tc-` prefix. |
| `task_id` | | |
| `author` | actor | |
| `body` | markdown string | Non-empty, at most 32 000 bytes. |
| `refs` | `[ThreadLink]` | Typed references, validated exactly the way a thread message's links are — see [Typed references](#typed-references). |
| `created_at` | RFC 3339 UTC | |

### Event

| Field | Wire | Notes |
| --- | --- | --- |
| `id` | `"te-01K5Z…"` | Same mint, `te-` prefix. |
| `task_id` | | |
| `at` | RFC 3339 UTC | |
| `actor` | actor | |
| `kind` | `created` \| `assigned` \| `unassigned` \| `moved` \| `labelled` \| `linked` \| `closed` \| `reopened` \| `dispatched` \| `tracked` \| `untracked` | |
| `payload` | object | What the kind needs. `created` carries `{title}`; `moved` carries `{from, to}`; `assigned` carries `{assignee}`; `labelled` carries `{added, removed}`; `linked` carries the link that was added; `closed` carries `{reason}`; `dispatched` carries `{workspace_id?, entity_id, agent_id, kind}`; `tracked` and `untracked` carry `{agent_id}` and, when the tracking was a consequence rather than a request, `{by: "assignment"}`. |
| `mentions_user` | `true`, absent otherwise | Since 1.27.0, an agent's `created` event has this when `create_task` was called with `mention_user: true`. Like the same flag on an agent comment, it asks the user to read or answer and counts only until the user's read mark passes the event. Other events omit it. |

### Actors

One tagged shape everywhere a person or an agent is named — `assignee`,
`created_by`, a comment's `author`, an event's `actor`:

```json
{ "kind": "user" }
{ "kind": "project_agent" }
{ "kind": "agent", "agent_id": "agent-01K5Z…" }
```

`project_agent` is only ever an **assignee**. An author or an actor is always
`user` or `agent`: by the time an agent of a project's conversation owner has
written something, it has an id, and that id already says it is a project agent
(the `project-` prefix). `assignee` may additionally be `null` — unassigned.

### Links

A task's links are what it is about in the repository and in Build:

```json
{
  "workspace_ids": ["ws-3f2a91c4"],
  "branches": ["build/tasks-board"],
  "commits": ["c8381faa…"],
  "conversation_ids": ["run-5d90b1e7"],
  "parent_task_id": null
}
```

The four lists are ordered by when each was added, deduped, and capped at 20
entries each. `parent_task_id` is a single task of the **same project**; a
task may not be its own parent, and a cycle is refused.

`conversation_ids` holds conversation owner ids (`run-…`), which is what
`agent.list`, `thread.page` and the rail are addressed by. A dispatch records
the conversation it delivered into here, so a task page can open the
conversation that is working it.

### Typed references

A comment's `refs` are `ThreadLink`s — the same enum a thread message carries,
with the same two-part fencing the Task Security Checklist records (controls 8
and 9):

1. **Shape**, by `validate_thread_links`: at most 20, a `file` path that stays
   inside a checkout, a line range that is ordered and non-zero, a `commit` that
   is 40 lowercase hex digits, a `worktree` id of the exact minted shape.
2. **Ownership**, scoped to the task rather than to a conversation: a `file`
   link is accepted only when the task links a workspace (its paths are
   checkout-relative and unresolvable otherwise); a `commit` must be one the
   task links; a `worktree` must be a checkout of a workspace the task links;
   `plan_stage`, `task_stage`, `run`, `implementation` and `recovery` are
   **refused outright** — they address the retired plan flow, which the tracker
   does not extend.

An agent-supplied ref that fails either half refuses the whole
`tasks.comment` / `comment_task` call by name. Nothing partially lands.

## Columns

Phase 1 has five fixed columns, in this order:

| Slug (stored) | Name (shown) |
| --- | --- |
| `backlog` | Backlog |
| `ready` | Ready |
| `in_progress` | In progress |
| `in_review` | In review |
| `done` | Done |

`status` is stored as the **slug string**, not as an enum, so a later
per-project column set is a record change and not a migration. Every verb that
takes a status accepts the slug or the display name, case-insensitively, and
normalizes to the slug; anything else is `invalid_params` naming the columns
that exist.

`tasks.columns {project_id}` answers the list. It takes a project so the verb
does not have to change when columns become per-project.

A closed task keeps its status. Closing does not move it to `done` and moving
it to `done` does not close it: one is "where is this on the board", the other
is "is this still open", and the SPA shows both.

## The verbs

All under `tasks.*`, registered in `api/v1` as a family of their own
(`bridge/src/api/v1/tasks.rs`) with typed params and results and a fixture
under `fixtures/api/v1/` per verb.

**A verb's `since` is the minor it actually shipped in, and a fixture must not
claim one the bridge never served.** The ten the tracker shipped with say
`1.3.0`. `tasks.track`, `tasks.untrack` and `tasks.for_agent` arrive later
and say the minor their own bump lands on — `versions.json` and `API_VERSION`
move together, and the new fixtures' `since` equals that number.

| Verb | Params | Result |
| --- | --- | --- |
| `tasks.list` | `{project_id, state?, status?, assignee?, label?, limit?, cursor?}` | `{tasks: [Task], user_session, next_cursor?}` — see [The user's session](#the-users-session) |
| `tasks.get` | `{task_id}` | `{task, timeline: [TimelineEntry]}` |
| `tasks.create` | `{project_id, title, body?, status?, labels?, priority?, assignee?, links?}` | `{task, dispatch}` |
| `tasks.update` | `{task_id, title?, body?, labels?, priority?, status?, state?}` | `{task}` |
| `tasks.assign` | `{task_id, assignee, note?}` | `{task, dispatch}` |
| `tasks.comment` | `{task_id, body, refs?}` | `{task, comment}` |
| `tasks.link` | `{task_id, workspace_id?, branch?, commit?, conversation_id?, parent_task_id?}` | `{task}` |
| `tasks.close` | `{task_id, reason?}` | `{task}` |
| `tasks.reopen` | `{task_id}` | `{task}` |
| `tasks.track` | `{task_id, agent_id}` | `{task}` |
| `tasks.untrack` | `{task_id, agent_id}` | `{task}` |
| `tasks.for_agent` | `{agent_id}` | `{agent_id, assigned: [TaskDigest], tracking: [TaskDigest]}` |
| `tasks.columns` | `{project_id}` | `{project_id, columns: [{id, name}]}` |

Notes on each:

- **`tasks.list`** answers newest first (`number` descending). Every filter is
  optional and they are ANDed. `state` absent means both; `assignee` takes the
  actor shape, plus the two words `"none"` (unassigned) and `"any"`. `state` and
  `status` are answered in SQL off the hoisted columns; `assignee` and `label`
  live inside the record and are applied to each row as that read goes. All four
  are params of the verb either way — a client sends them rather than filtering
  what it was given.
- **Paging `tasks.list`** (1.25.0, announced as `tasks.listPaged`, #85). `limit`
  (1 to 500) answers at most that many tasks, and `next_cursor` is present only
  when more follow. Handing it back as `cursor` with the same filter answers the
  tasks numbered below the last one the page answered, so a task filed while
  a client pages lands above the first page and never shifts a row across a
  cursor. The cursor is opaque and carries a digest of the list it was made in
  (the store's own key and the project's repository path, never its
  boot-local `proj-N`) and of the filter it was made under (state, status,
  assignee, label, as each means rather than as it was spelled). A cursor from
  another store or project is refused (`invalid_params`, "Build cannot
  continue this list: the cursor was made for another project or on another
  device."), one under any other filter as "Build cannot continue this list:
  the cursor was made for a different filter.", and one that cannot be read
  as "Build cannot read this cursor: ask for the list again from the start." No `limit` is the whole
  list, as before. A page reads only its own rows' timelines, so it holds the
  app lock for no longer than the whole list does. It reads at most four rows
  for each it may answer: under an assignee or label filter that passes over
  rows, a page can come back short, or empty, and still carry `next_cursor`,
  so a client walks on until `next_cursor` is absent.
- **`tasks.get`** answers the task and its whole timeline. Comments and events
  interleave into one ascending list ordered by `(created_at|at, id)` — ids are
  time-ordered, so equal timestamps still have one stable order. An entry is the
  record itself with one more key naming which it is — the **spread** form, not a
  nested one:

  ```json
  {"type":"comment","id":"tc-…","task_id":"task-…","author":{"kind":"agent","agent_id":"agent-1"},
   "body":"starting on this","refs":[],"created_at":"2026-08-21T10:01:00Z"}
  {"type":"event","id":"te-…","task_id":"task-…","at":"2026-08-21T10:01:00Z",
   "actor":{"kind":"user"},"kind":"moved","payload":{"from":"backlog","to":"in_progress"}}
  ```

  A comment stamps `created_at` and an event stamps `at`: one was written, the
  other happened. An event's `kind` is the event kind and has nothing to do with
  the `kind` discriminator inside an actor or an assignee.
- **`tasks.create`** mints the number inside the same transaction as the insert,
  writes a `created` event, and — when it was given an `assignee` — runs the
  whole of `tasks.assign` before answering. So `dispatch` on the result is the
  same shape `tasks.assign` answers, and `null` when nothing was dispatched.
  The assignee is read BEFORE the task is written: one this bridge cannot make
  sense of refuses the whole call rather than leaving a filed task nobody asked
  for. `note` is accepted and delivered; a form need not offer one, because on a
  create the body IS the task.

  A client should check `task.assignee` on the answer when it sent one. The v1
  facade parses params into the typed struct and hands the implementation the
  struct serialised BACK, which drops any field this bridge does not know — so
  a bridge older than this verb's `assignee` files the task unassigned and
  answers `ok`, with nothing in the refusal to say the choice went nowhere. That
  is true of every optional param on every v1 verb; it matters here because
  assignment is dispatch, and a dropped assignee means somebody walks away
  believing an agent is on it.
- **`tasks.update`** applies only the fields present. Each one that actually
  changes something writes its own event: `moved` for `status`, `labelled` for
  `labels`, `closed`/`reopened` for `state`. A `title`/`body`/`priority` change
  writes no event — the record's `updated_at` is the whole history those need.
- **`tasks.link`** takes one or more of its five keys and appends each,
  writing one `linked` event per link that was not already there.
- **`tasks.close`** on a closed task and **`tasks.reopen`** on an open one are
  `conflict`, not silent no-ops: the caller believed something that was not true.

Errors are `not_found`, `invalid_params`, `conflict` and `internal`, named the
way the workspace family names them.

`fixtures/api/versions.json` goes to `1.2.0` — a minor bump, because every verb
here is new and nothing existing changed shape. `supported_majors` is untouched.

## Assignment is dispatch

`tasks.assign` takes one tagged `assignee`. Assignment IS dispatch here, so a
single field says both who holds the task and where the work runs; there is no
second `dispatch` object for the two to disagree in. The five kinds:

| `assignee` | What happens | Stored assignee |
| --- | --- | --- |
| `{"kind":"user"}` | Nothing is dispatched. | `{"kind":"user"}` |
| `{"kind":"project_agent"}` | `project.ensure_conversation` on the task's project, an agent on it if it has none, then deliver. | `{"kind":"project_agent"}` |
| `{"kind":"agent","agent_id":…}` | Deliver into that agent's conversation. The agent must be on this task's project. | `{"kind":"agent","agent_id":…}` |
| `{"kind":"new_workspace","name"?,"isolation"?,"provider"?,"model"?,"effort"?}` | `workspace.create` in the task's project, `agent.add` on the workspace's conversation owner, deliver. | `{"kind":"agent","agent_id":…}` — the agent that was made |
| `{"kind":"new_agent","workspace_id","provider"?,"model"?,"effort"?}` | `agent.add` on that workspace's conversation owner, deliver. | `{"kind":"agent","agent_id":…}` |

`new_workspace` defaults its `name` to the task's title and its `isolation` to
the project's own setting — it passes no `isolation` at all when none was asked
for, which is how `workspace.create` reads "the project's".

**The agent's choice is spelled `provider` on the wire and `harness` in a
tool**, which is the rule this codebase already follows rather than a new one:
`agent.add` takes `provider`, and the project surface's `add_workspace_agent`
takes `harness` and maps it (`AgentChoiceArgs`, `app/projects/agent_writes.rs`,
which says so in as many words — the model is told what it is choosing between,
the daemon is told which field it is). So `tasks.assign` takes `provider` and
the SPA passes its `agentChoiceParams` straight through, while `assign_task`
takes `harness` and the bridge maps it before `agent.add`. Neither surface
accepts the other's spelling: one word per surface, decided here, is what keeps
the two from drifting into both.

Absent is absent, not null. A choice key that was not asked for is left OUT of
the params `agent.add` is called with, because that verb reads the PRESENCE of a
key to tell "run it on this" from "run it on whatever the workspace runs on".

An optional `note` on `tasks.assign` is extra instruction text delivered under
the task. It is not stored on the task: the task's body is the task, and a
hand-off note belongs in the conversation it was said in.

Assigning over an existing assignee replaces it and writes one `assigned` event.
Assigning `null` unassigns and writes `unassigned`; it dispatches nothing and
stops nothing that is already running.

### `new_workspace` waits for its checkout

Cutting a workspace is seconds to minutes of git on a real repository, and
`workspace.create` already hands that work to the deferred drain and answers
`{workspace_id, pending}` under the lock. A workspace that is still provisioning
has no conversation owner — `workspace.ensure_conversation` refuses one that is
not `ready` — so the agent cannot be added until the checkout exists.

So `tasks.assign` with `new_workspace` **defers**, exactly the way
`workspace.create` does: it hands the cut to the drain with the rest of the
dispatch hung off the end of it, and the client's reply is the one the drain
publishes once the agent has been made and the task handed over. The reply
shape is unchanged — `{task, dispatch}` with every id filled in — because the
client never sees the placeholder. The other four kinds do not defer; they are
a SQLite write and an in-memory lookup.

Nothing is written to the task until the checkout exists. A cut that fails
leaves the task exactly as it was: unassigned, in its old column, with no
`assigned` or `dispatched` event. A task must never name an agent that was
never made.

The wrapper is why this is reuse rather than a fork: `run` and `invalidate` are
`workspace.create`'s own, untouched, and only `settle` — which runs with the
mutex retaken and the checkout on disk — belongs to the tracker.

### Reuse, not a fork

Every one of these goes down the code path that already exists:

- the workspace is cut by `AppState::workspace_create`, the same call
  `workspace.create` and the project agent's `create_workspace` make;
- the agent is added by `AppState::project_agent_add_workspace_agent`, which
  mints the workspace's conversation owner with
  `workspace.ensure_conversation` when it has none and then calls `agent.add` —
  the same call `add_workspace_agent` makes;
- the project's conversation is `AppState::project_ensure_conversation`;
- the delivery is `AppState::post_from_agent_to_agent` /
  `AppState::thread_post`, the same send `message_agent` and
  `message_workspace_agent` make, so it is durable, it gets an operation
  receipt, it skips `note_user_message`, and it starts the agent exactly the
  way any other queued turn does.

The scope check is the project agent's own: a workspace or an agent that is not
in this task's project is refused in the same words
(`workspace … is not in project …`, `agent … is not in project …`).

### The dispatch result and the events

A dispatching assign answers

```json
{
  "task": { … },
  "dispatch": {
    "kind": "new_workspace",
    "workspace_id": "ws-3f2a91c4",
    "entity_id": "run-5d90b1e7",
    "agent_id": "agent-01K5Z…",
    "operation_id": "op-…"
  }
}
```

and writes two events: `assigned`, then `dispatched` carrying the same four ids.
`operation_id` is the receipt for the **delivery** — the turn the agent was
actually given — and never the workspace cut's.
It also links **where the work is** — the `workspace_id` and the `entity_id` go
onto `links.workspace_ids` and `links.conversation_ids` — and those write no
separate `linked` events, because `dispatched` already says it.

This holds for every dispatching kind and not only the two that create
something. Handing a task to an agent that already exists links the workspace
that agent is working in, looked up from its conversation, because a task that
recorded only the conversation would name who is on it and not where the code
is — which is the question anybody reading the task later asks. The project's
agent links no workspace: it works in the repository itself. **Unassigning
unlinks nothing.** Where a task was worked is a fact about its history, and
handing it back does not unmake the checkout.

Which is what makes self-assignment worth asking an agent for, and the
task-tools prompt does: an agent that picks up a task nobody handed it
assigns it to itself, and the checkout is recorded by that alone. `link_task`
is then left with what assignment cannot know — the branch cut for the task,
each commit that lands for it, and any second workspace.

A dispatching assign also **moves the task to `in_progress`** and writes a
`moved` event, when the task is open and its status is `backlog` or `ready`.
A task already at `in_progress`, `in_review` or `done` is left where it is:
the board position was set deliberately and a reassignment is not a reason to
rewind it.

### The envelope

The delivered message is an ordinary conversation message on the user's side of
the conversation — that is the side an instruction arrives on whoever wrote it —
carrying an **task envelope** the way a hand-off carries `from_agent`:

```json
{
  "id": "message-9",
  "role": "user",
  "from_task": {
    "task_id": "task-01K5Z…",
    "number": 12,
    "title": "Kanban drag does not persist",
    "links": { "workspace_ids": [], "branches": [], "commits": [],
               "conversation_ids": [], "parent_task_id": null }
  },
  "from_agent": { "id": "agent-01K5Y…" },
  "body": "The wire-facade agent assigned you task #12 — Kanban drag does not persist"
}
```

`from_task` is optional and absent on every other message, so a client that has
never heard of it reads the conversation exactly as it always has. `from_agent`
is present when an agent did the assigning and absent when the user did.

**An assignment is a notice, not the task.** The body is one line naming who
assigned what — `{assigner} assigned you task #12 — <title>` — with the note
under it when one was given, and nothing else. The assigner is "The user" for a
human, and for an agent the name its conversation goes by: `The wire-facade
agent`, `The Build project's agent`, or `Agent <id>` when Build cannot name the
owner.

The task's own text is never copied into the conversation. A copy goes stale
the moment anybody edits the task, and it sits in the agent's context being
compacted away before the work has begun — so the agent reads the task with
`get_task` when it is ready to start, which is also when the task is current.
The envelope carries no body for the same reason: it identifies the task for a
client's card, and the card draws the number and the title.

The delivery prompt the harness actually reads gains one line, beside the
existing "These messages came from agent `…`" and "The user sent this from
workspace `…`" lines (`bridge/src/operation.rs`). It does not repeat the
notice — the body is already the first thing in that prompt — so it carries
what the notice has no room for, the id `get_task` needs:

> The task is `task-01K5Z…` — read it with `get_task` before you start.
> Comment your progress on it with `comment_task`, and move it to In review
> with `move_task` when you report Complete.

## The MCP tools

Eight tools, on **both** the coding surface and the project surface:

`list_tasks`, `get_task`, `create_task`, `comment_task`, `assign_task`,
`move_task`, `close_task`, `link_task`, `track_task`, `untrack_task`.

Each is a thin wrapper over the verb of the same shape. The bridge knows who is
calling, so:

- **no tool takes a project**. The scope is the project the calling agent's
  conversation owner is bound to — a coding agent's workspace's project, a
  project agent's own. A call carrying a project id is parsed as though it had
  not, exactly as the project surface's tools already are.
- **no tool takes an author or an actor**. The calling agent is the author of
  every comment it writes and the actor of every event it causes.
- a task in another project is `unknown task_id`, and an assignee in another
  project is refused by name. The gate is on the socket (`allowed_on`) as well
  as in the tool list a session is shown, so a harness writing its own frames
  reaches no further than one that reads the list.

`track_task` and `untrack_task` take an `task_id` and nothing else. Which
agent is tracking is the caller — a tool cannot subscribe somebody else, the
way it cannot sign a comment as somebody else — so there is no `agent_id` for a
call to carry and none for it to get wrong.

`assign_task` takes the same five assignee kinds the verb does, so an agent can
hand work to a named agent, to the project's agent, back to the user, or to a
workspace and agent it asks Build to create. On the two creating kinds it spells
the agent's choice `harness`, the way every other tool on these surfaces does,
where the wire verb spells it `provider` — see
[Assignment is dispatch](#assignment-is-dispatch).

`move_task {task_id, status}` is `tasks.update` narrowed to one field,
because moving a card is what an agent does and offering it the whole update
would invite it to rewrite a title it was not asked about.

### The prompt note

One shared block appended to every template that carries these tools, the way
`MESSAGE_AGENT_NOTE` is appended (`bridge/src/templates.rs`) — so the wording
cannot drift between surfaces and a project that overrides one template
overrides only that one. It says:

- when a message hands you a task, that task is the work: comment your
  progress on it rather than only reporting at the end;
- move it to **In review** when you report Complete — you are saying the work is
  ready to be looked at, not that it is accepted;
- hand work off by **assigning** the task, not by messaging: an assignment
  delivers the task and starts the agent, and leaves a record on the task that
  a message does not;
- file a task for follow-up work you find and do not do. A task is cheap and
  the thing you noticed is otherwise only in your conversation.

## Tracking

A task that several agents are working around is only useful if they hear
about it. Tracking is how: an agent says it wants to know, and every later
change to that task is delivered into its conversation as a message. No
polling, and no staying awake — a delivered notice starts the agent's turn like
any other delivered message, so an idle agent wakes to it.

### Who is tracking

`trackers` is a list of agent ids on the task, ordered by when each started,
deduped, capped at 50. A task nobody watches answers `[]` rather than
omitting the key.

An agent joins the list three ways:

- **It asks**, with `tasks.track` / the `track_task` tool.
- **It is assigned the task.** Assignment is dispatch, so the agent that gets
  the work is the agent that most needs to hear about it; adding it is not a
  courtesy but the thing that makes the hand-off two-way. The `tracked` event
  for this carries `{by: "assignment"}`, so a timeline reader can tell a
  request from a consequence.
- Nothing else. Commenting on a task does not subscribe you to it: an agent
  that answers a question and moves on should not be woken for the next month.

An agent leaves only by asking (`tasks.untrack` / `untrack_task`). **Being
unassigned does not untrack**, which is deliberate: handing work on is exactly
when the previous holder still wants to know how it went, and an agent that
does not can say so in one call.

Tracking a task that is already tracked, and untracking one that is not, both
answer the task unchanged and write no event. This is a set, and saying a
thing twice is not a second fact.

### Following in the same call

Every task write an agent makes through its tools takes an optional
`track` boolean: "and put me on this task's trackers". Idempotent, like
`track_task` — asking twice is not two trackers and writes no second event.

It defaults to **false** everywhere except `create_task`, where it defaults to
**true**. An agent that moves somebody else's card in passing has not asked to
hear about it ever again; an agent that FILES a task almost always wants to
know how it goes, and the one that filed and assigned twelve in an afternoon
heard nothing about any of them.

The write's answer carries the task with its `trackers` as they now stand, so
following costs no second call and no second read. Honoured once, after the
write, because the task a create follows is one that did not exist when the
call was made — the only place every write can name its task is its answer.
If the tracking fails where the write did not, the write still stands: it is
durable before this runs, and reporting the call as failed would invite the
agent to make it twice.

**Tools only.** The `tasks.*` wire verbs are the board, and the board is a
human, who is not an agent and cannot be a tracker. They carry no such field
and ignore one, the way they ignore any unknown field.

### What a change delivers

Every change to a tracked task delivers one notice per tracker: a status or
column move, a state change, a title, body, labels or priority edit, a new
comment, an assignment, a link. The notice is an ordinary conversation message
on the user's side — the side an instruction arrives on whoever wrote it —
carrying:

- **`from_build: true`**, the mark the restart notice already uses. It says the
  daemon wrote this and nobody is waiting on an answer to it. A client draws it
  as Build's own words rather than as the reader's.
- **`from_task`** — the same envelope a dispatched task carries: `{task_id,
  number, title, links}`. It is what lets a client draw the notice as a card on
  the task and link `#13`.
- **`task_notice`** — what happened, structured, so a client draws one line
  that deep-links the task or the comment instead of parsing it back out of
  prose:

  ```json
  {
    "actor": { "kind": "agent", "agent_id": "agent-01K5Z…" },
    "action": "moved",
    "from": "in_progress",
    "to": "in_review"
  }
  ```

  `action` is one of `commented`, `moved`, `assigned`, `unassigned`, `created`,
  `closed`, `reopened`, `edited`, `linked` — a slug, the way a column is.
  `comment_id` rides `commented` and is what links the comment rather than the
  task; `from`/`to` ride `moved` as column slugs; `assignee` rides `assigned`.
- **A one-line body** saying the same thing in prose, for a harness — which
  gets the body or nothing — and as the fallback for a client that has not
  learned `task_notice`: `The wire-facade agent moved #13 Kanban drag to In
  review`. An agent is named by the workspace it works in rather than by its
  id, because the reader wants to know which colleague moved the card. When
  the change is a comment, the comment's body follows underneath, because the
  whole point of hearing about a comment is reading it.

**A tracker is not the assignee, and the notice must not read as a brief.** The
delivery envelope tells an agent handed a task to read it with `get_task`
before it starts, to comment its progress and to move the card to In review
when it reports Complete. A notice wears `from_task` too, so a watcher was
getting that same instruction — which is telling it to take over work nobody
gave it. The assignment wording is for the assignee's dispatch message only;
a notice says what it is, that nothing is being asked, and how to stop them.

**An agent is never told about its own change.** The actor is excluded from the
delivery, always. An agent that moves a card and is then woken to be told it
moved a card would answer its own message, and two agents each tracking the
other's task would do it forever.

A change that alters nothing delivers nothing, for the same reason it writes no
event: moving a card to the column it is already in is not news.

Delivery is durable and goes down the path every other delivered message goes
down — the same queue, the same receipt, the same start. It skips
`note_user_message`, as an agent-to-agent hand-off does: Build telling an agent
something is the work happening, not somebody speaking to the human, and it
must not move the inbox anchor under a reader.

### An agent says what it did

Separately from tracking, and for a different reader: **every task write an
agent makes through its tools also posts one message into that agent's own
conversation**, authored by the agent.

The reason the two are separate mechanisms. A tracking notice is Build telling
somebody ELSE what happened, and it is marked `from_build` because nobody is
waiting on an answer to it. An action message is the AGENT saying what it just
did, in its own conversation, to the person reading that conversation. It is
role `agent` and carries no `from_build` mark, because it is not Build's
sentence — it is the agent's, and a reader scrolling the conversation should see
"Commented on #13 …" between the agent's other words rather than as something
the system interjected.

It is an ordinary agent message and not an activity row, so it survives every
detail mode: a conversation read at its coarsest still shows what the agent did
to the board, because that is a thing the agent did and not a tool call it made
on the way.

The message carries:

```json
{
  "role": "agent",
  "body": "Commented on #13 Task tracking: an agent that tracks a task …",
  "task_action": {
    "action": "commented_on",
    "task_id": "task-01K5Z…",
    "number": 13,
    "title": "Task tracking: an agent that tracks a task …",
    "comment_id": "tc-01K5Z…"
  }
}
```

`action` is one of `created`, `assigned`, `moved`, `closed`, `reopened`,
`commented_on`, `linked`, `updated` — a slug, the way a column is, so a client
renders the label and the bridge does not decide the wording twice. `comment_id`
is present only on `commented_on`, and is what lets a client deep-link the
comment rather than the task.

**One message per write, never two.** A call that relabels and moves is one
thing the agent did, and two messages about it would be two lines in a
conversation for one action. This is why the message is posted in
`commit_task_write` — the one funnel every tracker write already goes through —
rather than in each verb.

A create is the one write that does not pass through that funnel, because its
number is minted inside the insert's own transaction; the create path calls the
same hook by hand, and the rule is unchanged. There is no
`created_and_assigned`: a create and an assignment are two separate writes, and
`create_task` takes no assignee at all, so no agent can reach the pair in one
call.

**Only when the actor is an agent with a conversation.** A human moving a card
on the board is already looking at the board; posting into a conversation nobody
is reading to tell them what they just did on screen is noise. So the api path
posts nothing, and the check is on the actor rather than on which verb was
called — the same verb serves both.

The message is in the ACTING agent's conversation and nobody else's. An
assignment posts "Assigned #13 to …" for the agent that assigned; what the
assignee gets is the dispatched task itself, which it was already getting.

Its shape is pinned in `fixtures/api/v1/thread.page.json` with the notice, for
the same reason and in the same file.

### The Complete reminder

An agent reports Complete and walks away from three open tasks assigned to it.
Nobody is told, the tasks sit in In progress, and whoever assigned them finds
out by going to look.

So on **Complete**, and only on Complete, Build delivers one more `from_build`
message into that agent's own conversation, naming every task assigned to it
that is **still its to finish**, and saying the three ways out: finish it,
comment where it got to, or hand it back with a comment saying why.

Still its to finish means the column is `backlog`, `ready` or `in_progress`.

- **In review is not held.** In review means the agent has reported Complete
  and the work is ready to be looked at; whether it is done is somebody else's
  call. A task sitting there is waiting on a reviewer, not on the agent, and
  telling the agent otherwise asks it either to redo finished work or to game
  the column. Done and closed are excluded for the same reason and more
  obviously.
- **Not on Blocked or Waiting.** Both are the agent saying it cannot finish,
  which is already an answer about the work. A list of what it has not finished
  would be telling it what it just told us.
- **Not when it holds nothing.** Silence is the right answer there.
- **It names every task**, with the column each is in, rather than counting
  them. "You still hold 3 tasks" makes the agent go and look, and the looking
  is the part Build can do.
- **Once per set, not once per Complete.** The reminder is delivered as a turn,
  so an agent that answers it reports Complete again — which is another
  reminder, which is another answer. A second Complete holding exactly what the
  agent was last told about says nothing; a set that has changed is news again.
  What each agent was last told is remembered in memory and forgotten on
  restart, which is the right way round: the reminder catches an agent walking
  away from work inside a session.

It runs after the report's own automatic activity, so a task the same
Complete handed on to In review is described as it now stands — which is also
what keeps it out of the list.

### Where the notice's shape is pinned

Both the tracking notice and the action message live in
**`fixtures/api/v1/thread.page.json`**, beside the other message shapes — not
in `events.json`, and not in a `thread.json` (there is no such fixture; the
message shape is `thread.page`'s).

`events.json` holds what the bridge pushes on a session: `changes`,
`board.changed`, `entity.changed`, `term.*`, `rtc.ice`. Its contract test
matches on the event's `type` and panics on anything it does not recognise
(`the bridge sends no such push`). A notice is not a push: it is a message on a
conversation, and a client reads it out of `thread.page` with every other
message. Putting it in `events.json` would either break that test or make the
file mean two things.

So a `thread.page` item carries the example, and the same file is where
`from_build` is pinned for the first time — the restart notice has been sending
it since `486001f8` without a fixture saying so.

### The per-agent read

`tasks.for_agent {agent_id}` answers what one agent is on:

```json
{
  "agent_id": "agent-01K5Z…",
  "assigned": [{ "task_id": "task-01K5Z…", "number": 13, "title": "…",
                 "state": "open", "status": "in_progress",
                 "updated_at": "2026-09-20T15:04:00Z" }],
  "tracking": [ … the same digest … ]
}
```

A digest and not the whole task: this is a list somebody scans, and the body
of thirty tasks is not a list. A task the agent both holds and tracks — which
is every assigned one — appears in both, because the two questions are
different and a client showing one should not have to know about the other.

Both lists are newest-updated first. Scoped to the agent's own project, which
the bridge resolves from the agent rather than taking as an argument.

### Scope

An agent may track only tasks of its own project, and may track only itself:
`tasks.track` names an `agent_id`, and a tool call's is forced to the caller.
The wire verb is the user's, so it may name any agent of the task's project
and refuses one outside it by name — the same refusal every project-scoped
handler gives.

## Watching

Tracking above is the agents'. Watching is the person's: which tasks are in
the USER's inbox, how far they have read one, and whether the row is cleared.
The two are the same idea and deliberately not the same field — an agent
tracker gets a notice delivered into its conversation and its turn started; the
user's watch puts a row in a list. `trackers` stays what it has been since
1.5.0, an array of agent-id strings, and the user's watch is `watched: bool` on
the task beside it. Conversations gain the same `watched` on the agent record.

### What the user watches without asking

- **A task they filed, commented on, or were assigned.** Filing and saying
  something are caring about it; being handed one is being asked about it.
- **A conversation they made.** Every agent added from the UI is watched; one
  an agent added (`made_by_agent: true` on `agent.add`) is not, because an
  agent's own helper is that agent's business.
- **A task an agent filed**, when the device setting `watch_agent_filed_tasks`
  is on — which is the default, so a device that has never been asked behaves
  as it did before watching existed.
- **Anything an agent asked to be seen** with `notify_user: true`, on
  `create_task`, `comment_task`, `assign_task` or `agent.add`. This is the
  case the feature is for: the user asks an agent to open a task to workshop
  something, and it appears in their inbox without them going to find it.
- **A task an agent filed with `mention_user: true`.** It also appears in
  the user's Needs you until they read its `created` event, even if the device
  setting to watch agent-filed tasks is off.

Nothing else. An agent filing among agents on a device that has turned the
setting off, and an agent an agent spawned, are not the user's business until
somebody says they are.

### What needs the user (#144)

A watched task is one the user can see; it needs them only when:

- it is **assigned to the user**, or
- an agent's `created` event they have not read **mentions them**
  (`mention_user` on `create_task`, kept as `mentions_user` on the event since
  1.27.0), or
- an agent comment they have not read **mentions them** (`mention_user`, kept
  as `mentions_user` on the comment), or
- an agent comment they have not read **asked them** (`notify_user`, kept as
  `notifies_user` on the comment since 1.23.0).

The In review column alone no longer qualifies: agents review each other's
work there, and a review round between agents has nothing for the user to
decide. A task in review is assigned to whoever is reviewing it, and its card
says so ("In review · Astra reviewer", "In review · you").

A bridge announces the narrow rule as `tasks.commentUserNotifies`, and the
created-event flag as `tasks.createdUserMentions`. Against a bridge without
the narrow-rule name, a client keeps the earlier rule — In review, or any
unread agent comment on a watched task — because such a bridge does not
record which comments asked. A client that reads the created-event flag uses
the event's own id and the same read mark as comments; it needs no separate
state or clock. Older clients ignore the optional event field.

The inbox and the Tasks tab's Needs you read this from the same cached
records: the task (its `watched`, `assignee` and `read_through`) and its
timeline. Neither waits on the board's `tracker_task` feed row, whose unread
count is re-read only with the board and can lag a comment an `tasks` push has
already cached. Nor does the row stand in for the watch: after Stop watching,
the cached task says `watched: false`, and the task leaves both at once.

### The verbs

| Verb | Params | Result |
| --- | --- | --- |
| `tasks.watch` | `{task_id}` | `{task}` |
| `tasks.unwatch` | `{task_id}` | `{task}` |
| `tasks.read_through` | `{task_id, event_id}` | `{task}` |
| `tasks.dismiss` | `{task_id}` | `{task}` |
| `conversation.watch` | `{entity_id, agent_id}` | `{agent_id, watched}` |
| `conversation.unwatch` | `{entity_id, agent_id}` | `{agent_id, watched}` |

All at `1.9.0`, with `API_VERSION` and `versions.json` moving together.

`tasks.watch` and `tasks.unwatch` write a `watched` / `unwatched` event, and
only when something actually changed: watching what is already watched answers
the task unchanged and writes nothing, as tracking does. **Mute is unwatch**
on a task — there is no third state, and the row's absence from the next
`board.list` is the whole of the answer.

`tasks.read_through` never moves the mark backwards, and writes no event:
reading is not something that happened TO the task, and a timeline that
recorded every scroll is one nobody could read. `tasks.dismiss` is Done —
it stores the newest entry's id as `dismissed_through`, and the next thing that
happens is past it, so the row comes back on its own.

Both marks are entry ids, and a timeline holds two kinds: comments are `tc-…`
and events are `te-…`. **They are compared without the prefix.** The ULID after
it is time-ordered; the prefixes are not, and comparing them whole would put
every comment below every event — a mark left on an event would hide every
comment made after it.

The user's own entries are left out of the unread count. The badge is a list of
things asking for their attention, and a count that went up when they commented
would be telling them about themselves.

### The inbox row

`board.list` carries one row per watched task in `items`, the same list the
conversations are in, interleaved by `anchor`:

```json
{
  "kind": "tracker_task",
  "task_id": "task-01K5Z…",
  "number": 12,
  "project_id": "proj-1",
  "title": "Kanban drag does not persist",
  "status": "in_review",
  "assignee": { "kind": "agent", "agent_id": "agent-01K5Z…" },
  "assigned_to_user": false,
  "last_event": { "text": "New comment from the rail agent",
                  "actor": "the rail agent", "at": "2026-09-19T10:12:00Z" },
  "anchor": "2026-09-19T10:12:00Z",
  "last_activity": "2026-09-19T10:12:00Z",
  "unread": 3,
  "muted": false,
  "done_until_next": false
}
```

`kind` is `tracker_task` and not `task`: the feed already spends `task` on
the multi-stage kind, which routes to another page. `anchor` and
`last_activity` are both the instant of the last event, so the interleave with
the conversation rows needs no special case. `muted` is always false, for the
reason above. `unread` is a COUNT here where a work row sends a flag — a
task's badge is how much has happened.

`last_event.text` is composed by the bridge, in the reader's voice of the same
line #61 sends an agent: the same composer, two voices. The agent's line names
the comment id and the tool to read it; the reader's says "New comment from the
rail agent" and "Moved to In review by you". One function decides the facts, so
the two cannot drift; the voice is the only difference. (The reader's voice is
what a task page's timeline sentences should use as well.)

`session.hello` states `"tasks": { "attachments": true, "watching": true }`, so
a client gates its inbox on the capability rather than on a version compare.

## Dashboard task sections

The Dashboard presents open tasks in **Active** or **Backlog**. Active has two
groups: **Working** for a task held by an agent whose cached feed says it is
working now, then **Assigned** for every other held task, including tasks held
by the user or by an agent absent from the current feed. Backlog is a flat list
of open tasks with no holder. The board column does not decide which section a
task belongs in. Both Active groups and Backlog put higher priorities first and
keep the task list order within a priority. An empty Active group is hidden.
Needs you remains a separate lens and can repeat a task from Active or Backlog.
Done remains the completion lens below.

## The user's session

The dashboard's Done section is "Done since you left" (#106): every task in
Done whose `done_at` is at or after the moment the user left. The bridge keeps
one device-wide summary of the user's session and `tasks.list` carries it
beside the tasks, so the section paints from the cached list and needs no
cached timelines.

```json
"user_session": {
  "session_started_ms": 1758272400000,
  "last_activity_ms": 1758276720000,
  "previous_session_ended_ms": 1758218400000,
  "gap_ms": 21600000,
  "now_ms": 1758277020000
}
```

`now_ms` is the bridge's clock when it answered. A client reads from it
whether the user was away AT THAT ANSWER (`last_activity_ms` a gap or more
before it), and nothing more: it never adds the time since, and never reads its
own clock, which may be hours out. Time passing since an answer says nothing
about the user, who may have been busy on another client all along.

- **Activity** is the user acting: the client verbs in
  `rpc::USER_ACTIVITY_VERBS`, which are messages, task writes, read marks
  (`tasks.read_through`, `entity.seen`), Resume, Stop and opening a terminal
  (`agent.start`, `agent.interrupt`, `term.create`, stamped where the frame is
  dispatched because they bypass `dispatch`), and the other user-initiated
  writes. Refusals are not activity. Reads, subscriptions, acks, resizes and
  re-attaches are not in it, and neither is `term.input`, which a terminal
  also sends by itself. Agents act through MCP and never reach it.
- **Arriving** is activity too: `user.present` (no params; answers
  `{user_session}`) records it on the bridge's clock. The client sends it to
  every connected bridge that announces `tasks.doneSinceLeft` when the app
  opens in a focused window, when the window is focused again (or its tab
  shown while focused), on a pointer, key or wheel input, and on navigation;
  at most once a minute per bridge (`spa/src/core/userPresence.js`). A
  reconnect, a list read or a push is never an arrival, and an arrival a
  bridge could not be told of is dropped after two minutes, so a bridge that
  reconnects at 3am is not told the user arrived at 3am. A send that waited on
  the bridge's greeting checks again when the greeting settles: still fresh,
  and the window still visible and focused.
- **Automatic read marks** (a chat or task page marking what arrives on
  screen, rather than the user sending, moving or opening something) are sent
  only while the document is visible and focused (`document.hasFocus()`). A
  window left showing a chat overnight neither reads the user's messages for
  them nor keeps their session alive; when focus returns, what is on screen is
  marked then (`spa/src/core/readerPresence.js`).
- **A session** ends after `gap_ms` (6 hours) without activity. The update is
  #98's session function with that gap; when a new session starts,
  `previous_session_ended_ms` becomes the old `last_activity_ms`.
- **Pushed** when a new session starts: an `tasks` change on every project
  (empty `task_ids`), so every client re-reads a list and its session. A
  laptop holding the old session does not infer an absence the user spent on
  their phone. Activity inside a session pushes nothing, because nothing a
  client decides depends on it: a client infers no silence from a snapshot
  aging, so a laptop that last read at 09:00 still lists the overnight work at
  15:00 while the user spent the day on their phone.
- **Stored** as one `meta` row, rewritten on each change. The row is
  authoritative for the interval it covers, since much of what made it (read
  marks, which keep only their latest) is not in the store to replay; a boot
  starts from it and folds only the stored user actions after its last
  activity (task events and comments by the user, messages the user sent),
  which are what a failed write missed. A store with no row (a bridge from
  before this shipped) replays them all.
- **Cutoff** (client): `previous_session_ended_ms`. If the answer itself
  shows the user away (a list read that landed before the arrival did),
  `last_activity_ms`, unless that silence was already longer than 96 hours,
  when nothing is listed until the arrival is recorded. If there is no
  earlier session, or it ended more than 96 hours before this one started, the
  cutoff is this session's start: a blank slate that fills as work finishes.
  The cap measures the absence, not the time since it, so a three-day weekend
  stays listed all through the day back.

Clients gate on the `tasks.doneSinceLeft` capability name; without it the
section stays "Done", the last 24 hours read from cached timelines.

## Push

Tasks push over the existing changes subscription
(`bridge/src/changes.rs`) as a new `Kind::Tasks`, wire-spelled `"tasks"`,
scoped to the **project** entity:

```json
{ "subscription_id": "s-inbox",
  "scope": { "kind": "entity", "id": "proj-1" },
  "kinds": ["tasks"],
  "mode": "realtime" }
```

An item names the tasks that moved:

```json
{ "entity_id": "proj-1", "tasks": { "task_ids": ["task-01K5Z…"], "truncated": false } }
```

Content-free beyond the ids, like every other signal Build sends about work it
cannot read: a client refetches `tasks.list` or `tasks.get`. Past 200 ids in
one un-flushed window the item is `truncated` and the ids are dropped, which
means "refetch the list", exactly as a `files` item's truncation does.

`tasks` is not a worktree kind: it is not paced by the settle window, it needs
no filesystem watcher, and it never makes a subscription answer `polled`.

A subscription scoped `{"kind":"all"}` that names `tasks` receives every
project's items, each with `entity_id` set to that project's `proj-N` id — `all`
covers whatever is noted, and nothing about the project-scoped spelling above is
a restriction. Under `all` an `tasks` item is therefore the one item whose
`entity_id` is a project rather than a work entity.

An `tasks` note fires the **subscription path only**. It emits no legacy
`entity.changed` and does not bump the board revision: a client in legacy mode
has no tasks surface to refetch, and bumping the board on every comment would
repaint the feed for something the feed does not show.

Every mutation — create, update, assign, comment, link, move, close, reopen, and
each automatic activity below — notes its project once, after the write lands.
A `changes` subscriber also keeps hearing `thread` on the conversation a task
was delivered into: that is the same message arriving, seen from the other side.

## Automatic activity

Two things happen without anyone asking.

**An agent that holds a dispatched task reports Complete.** The task moves to
`in_review` with a `moved` event whose payload says `{"by": "report"}`. It holds
the task when the task's `assignee` is `{"kind":"agent"}` naming that agent,
or the task's `links.conversation_ids` holds that agent's conversation owner
and nobody else holds it. One report moves at most one task — the one most
recently dispatched to that agent. A Blocked or Failed report moves nothing:
blocked is not ready to look at, and a board that said it was would be lying in
the direction that wastes a reviewer's time.

**A report does not comment.** It used to: the report's body was copied onto the
task. That made every end-of-turn report a task comment, including four an
agent wrote answering a reminder it could not silence — five comments on one
task in three minutes, none of them written to it. **A conversation message is
never a comment on a task**, whatever it mentions, whoever it is addressed to
and whatever its viewing context names. `comment_task` and `tasks.comment`
are the two things that write a comment and they are the only two; an agent
that wants its report on the task calls one of them, and the prompt asks it
to. The timeline still records the move, and names the agent that caused it.

**A workspace linked to a task is finished.** `workspace.finish` and the
legacy branch Finish routes close linked open tasks only when the workspace's
run is merged. An unmerged finish leaves them open. The close has a `closed`
event whose payload is `{"reason": "workspace_finished", "workspace_id": …}`.
It happens when Done is accepted, before the folder is removed; a later disk
failure does not undo the merged work or reopen its tasks.

Neither of these moves the inbox anchor or crosses a dismissal line. They are
the work happening, not somebody speaking to the human.

## Storage

Three tables, added to `bridge/src/store/schema.rs`, taking `SCHEMA_VERSION`
from 7 to 8. They are all `CREATE TABLE IF NOT EXISTS`, so a store written by an
older bridge gains them on the next open with no `ALTER` and no backfill.

```sql
CREATE TABLE IF NOT EXISTS tracker_tasks (
    id          TEXT PRIMARY KEY,
    project_key TEXT NOT NULL,
    number      INTEGER NOT NULL,
    state       TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    record      TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS tracker_tasks_number
    ON tracker_tasks(project_key, number);
CREATE INDEX IF NOT EXISTS tracker_tasks_by_project
    ON tracker_tasks(project_key, number DESC);

CREATE TABLE IF NOT EXISTS tracker_comments (
    id         TEXT PRIMARY KEY,
    task_id   TEXT NOT NULL,
    created_at TEXT NOT NULL,
    record     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tracker_comments_by_task
    ON tracker_comments(task_id, created_at, id);

CREATE TABLE IF NOT EXISTS tracker_events (
    id       TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    at       TEXT NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tracker_events_by_task
    ON tracker_events(task_id, at, id);
```

The design rules the rest of the store follows, applied here:

- **A record is a record.** A task, a comment and an event are each small,
  bounded, and read and written whole, so each keeps its serde shape in a
  `record` column. Only what is queried is hoisted into a column of its own:
  the project key and number (the list read and the number mint), the state and
  status (the two filters answered in SQL), and the timestamps (the ordering).
  An assignee and a label are read out of the record: hoisting a label list
  would mean a join table, which phase 1 does not need.
- **Scoped by project path, answered by project id.** `project_key` is the
  project's canonical repository path, not its `proj-N` id — the same choice
  `PersistedPlan` and `PersistedRun` make and for the same reason: a `proj-N` id
  is minted per boot from the config that restored it, and two boots can spell
  the same repository differently. The wire only ever carries `project_id`; the
  store resolves it to a path on the way in and back on the way out.
- **The number is minted in the insert's own transaction**, as
  `MAX(number) + 1` over the project, under the `IMMEDIATE` transaction every
  store write already takes. The unique index is the backstop.
- **Nothing is deleted in phase 1.** A task is closed, not removed, so a
  number is never reused and a timeline never loses an entry. Deleting a project
  deletes its tasks, the way it deletes its plans and runs.

### A different backing store later

The store surface is six methods, all of them whole-record:

| Method | What it is for |
| --- | --- |
| `create_tracker_task(draft, events)` | Mints the number inside the insert's own transaction and answers the task as stored. |
| `save_tracker_task_activity(task, comments, events)` | The record moved and the timeline says why, in one transaction. Empty slices are a plain save. |
| `load_tracker_task(task_id)` | One task, or `None`. |
| `list_tracker_tasks(project_path, filter)` | One project's, newest first, narrowed by state and status. |
| `load_tracker_timeline(task_id)` | Comments and events merged into one ascending list. |
| `delete_tracker_tasks_of_project(project_path)` | Only reached by project deletion. |

None of them takes SQL, a connection, or a row. Replacing SQLite with something
else — a service, a file per task, a git-backed store — is implementing those
six against something else; nothing above this line knows what is underneath it.

An append is idempotent by id (`INSERT OR IGNORE`), so a retry of a write whose
answer was lost adds nothing a second time.

## Scope rules, in one place

1. A task belongs to exactly one project and never moves between projects.
2. An agent reaches only its own project's tasks. Which project that is comes
   from its conversation owner's binding, never from a tool argument.
3. An assignee must be of the task's project: an agent on one of its
   workspaces, its project agent, or the user.
4. A link must be of the task's project: a workspace of it, a conversation
   owner bound to it, a parent task in it.
5. A comment's typed refs are fenced twice — shape, then ownership by the task.
6. The user, over the wire, may act on any project's tasks; `project_id` is a
   param there because the browser is not scoped to a project the way an agent
   is.

## What the SPA phase builds

Four surfaces, all under the project (`#/project/<project_id>`), which is
already where a project's workspaces live.

**The list.** Rows of `#<number>  title`, with the assignee, the labels, the
priority and the status column on each. Filters for state, status, assignee and
label, matching `tasks.list`'s params one for one so a filter is a param and
not a client-side pass over everything. A new-task composer: title, markdown
body, and the assignee picker.

**The kanban.** The same tasks laid out in columns from `tasks.columns`, in
that order, each card the compact form of a row. Dragging a card between columns
is `tasks.update {status}`; the card moves optimistically and the column
repaints from the push. A column is not a filter the user typed — it is what
`status` says — so an empty column is still drawn.

**The task page.** The title, the markdown body, the state and status, the
labels, the priority, the assignee, the links, and the timeline: comments and
events interleaved, ascending, each stamped with its actor and time. A comment
composer at the bottom. A delivered task's conversation is one press away
through `links.conversation_ids`.

**The assignee picker.** One control over all five kinds — the user, the
project's agent, any agent on any workspace of the project (grouped by
workspace, from `workspace.list` plus `agent.list`), a new agent on an existing
workspace, or a new workspace. Choosing one of the last two is where the
harness/model/effort selects appear. The control says what it is about to do —
"cut a workspace and start an agent on it" — because assigning starts work and
the user should not discover that afterwards.

**The agent's activity entry.** Per agent, what it holds and what it watches,
from `tasks.for_agent {agent_id}` — two digest lists, each newest-updated
first, each entry `{task_id, number, title, state, status, updated_at}`. An
assigned task appears in both lists; that is not a bug to de-duplicate, it is
the two questions being different.

**Push.** The project page subscribes `{scope: {kind: "entity", id: project_id},
kinds: ["tasks"]}` and refetches on an item. A task page open on a task
named in an item refetches that task.

**On a message.** A conversation message carrying `from_task` draws as a task
card — `#12` and the title — linking the task page. There is no fold, because
the envelope carries no body; what the reader wants beyond the title is the
task page, one click away. It is still a message and still reads in sequence;
the card is how it is drawn, not a separate kind of thing.

## Boundaries

Tasks do not replace the plan flow's documents and they do not become one. They
hold no stages, no approvals, no diff and no review state. A task says what
should be done and where the doing is happening; the doing itself is a
workspace, a conversation and a diff, each of which already has a surface.

Assigning starts an agent. Nothing else here does: closing a task stops
nothing, moving a card stops nothing, and unassigning stops nothing. Stopping an
agent is `agent.remove` and `workspace.delete`, which say what they take with
them.
