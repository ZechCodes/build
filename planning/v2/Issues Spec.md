# Issues

## Purpose

An issue tracker, one per project, that is where the user and the agents working
that project agree on what is being done. It reads like GitHub issues — a title,
a markdown body, a state, labels, an assignee, comments and a timeline — with a
kanban view over status columns.

What makes it Build's rather than a copy of GitHub's: **assigning an issue is
dispatching it**. An issue handed to an agent arrives in that agent's
conversation and the agent starts on it. There is no second step where somebody
turns an issue into work.

Agents are first-class here. An agent of a project can list, read, file, comment
on, move, close, link and assign issues in its own project, and can assign to
another agent of that project, to the project's own agent, to the user, or to a
workspace and agent it asks Build to create. It cannot reach another project's
issues however a call is spelled.

This document is phase 1: the bridge. The SPA phase follows from
[What the SPA phase builds](#what-the-spa-phase-builds).

## Not the plan flow

Build already has a table called `issues` and a family of `issue.*` / `plan.*`
verbs. Those are the retired **plan** flow — a goal, stage documents, approvals
and implementation runs. This is a different thing that happens to share the
English word.

Nothing here extends that flow. The tracker gets its own tables
(`tracker_issues`, `tracker_comments`, `tracker_events`) and its own verb family
(`issues.*`, which cannot collide with the singular `issue.*`). Everywhere else
in this document, "plan" means the legacy flow and "issue" means the tracker's.

## The record

### Issue

| Field | Wire | Notes |
| --- | --- | --- |
| `id` | `"issue-01K5Z…"` | ULID-style, the same mint agent ids use: 48 bits of milliseconds then 80 bits of randomness, Crockford base32, under an `issue-` prefix. Time-ordered, so ids sort the way the issues were filed. |
| `project_id` | `"proj-1"` | The project the issue belongs to. |
| `number` | `12` | Per-project, sequential from 1, for `#12`. Never reused. |
| `title` | string | Trimmed, non-empty, at most 200 characters. |
| `body` | markdown string | May be empty. At most 32 000 bytes, the bound a thread message already carries. |
| `state` | `"open"` \| `"closed"` | |
| `status` | `"backlog"` | The kanban column, as a **slug string** — see [Columns](#columns). |
| `labels` | `["bug", …]` | Free strings, deduped, trimmed, at most 20, each at most 40 characters. |
| `priority` | `"none"` \| `"low"` \| `"medium"` \| `"high"` \| `"urgent"` | `none` is the default and is a value, not an absence. |
| `assignee` | `null` \| assignee | See [Actors](#actors). |
| `links` | object | See [Links](#links). |
| `trackers` | `["agent-01K5Z…"]` | The agents watching this issue — see [Tracking](#tracking). Ordered by when each started tracking, deduped, at most 50. Always present; an issue nobody watches answers `[]`. |
| `created_by` | actor | Who filed it. |
| `created_at`, `updated_at` | RFC 3339 UTC | |
| `closed_at` | RFC 3339 UTC \| `null` | Set when `state` becomes `closed`, cleared on reopen. |

### Comment

| Field | Wire | Notes |
| --- | --- | --- |
| `id` | `"ic-01K5Z…"` | Same mint, `ic-` prefix. |
| `issue_id` | | |
| `author` | actor | |
| `body` | markdown string | Non-empty, at most 32 000 bytes. |
| `refs` | `[ThreadLink]` | Typed references, validated exactly the way a thread message's links are — see [Typed references](#typed-references). |
| `created_at` | RFC 3339 UTC | |

### Event

| Field | Wire | Notes |
| --- | --- | --- |
| `id` | `"ie-01K5Z…"` | Same mint, `ie-` prefix. |
| `issue_id` | | |
| `at` | RFC 3339 UTC | |
| `actor` | actor | |
| `kind` | `created` \| `assigned` \| `unassigned` \| `moved` \| `labelled` \| `linked` \| `closed` \| `reopened` \| `dispatched` \| `tracked` \| `untracked` | |
| `payload` | object | What the kind needs. `moved` carries `{from, to}`; `assigned` carries `{assignee}`; `labelled` carries `{added, removed}`; `linked` carries the link that was added; `closed` carries `{reason}`; `dispatched` carries `{workspace_id?, entity_id, agent_id, kind}`; `tracked` and `untracked` carry `{agent_id}` and, when the tracking was a consequence rather than a request, `{by: "assignment"}`. |

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

An issue's links are what it is about in the repository and in Build:

```json
{
  "workspace_ids": ["ws-3f2a91c4"],
  "branches": ["build/issues-board"],
  "commits": ["c8381faa…"],
  "conversation_ids": ["run-5d90b1e7"],
  "parent_issue_id": null
}
```

The four lists are ordered by when each was added, deduped, and capped at 20
entries each. `parent_issue_id` is a single issue of the **same project**; an
issue may not be its own parent, and a cycle is refused.

`conversation_ids` holds conversation owner ids (`run-…`), which is what
`agent.list`, `thread.page` and the rail are addressed by. A dispatch records
the conversation it delivered into here, so an issue page can open the
conversation that is working it.

### Typed references

A comment's `refs` are `ThreadLink`s — the same enum a thread message carries,
with the same two-part fencing the Issue Security Checklist records (controls 8
and 9):

1. **Shape**, by `validate_thread_links`: at most 20, a `file` path that stays
   inside a checkout, a line range that is ordered and non-zero, a `commit` that
   is 40 lowercase hex digits, a `worktree` id of the exact minted shape.
2. **Ownership**, scoped to the issue rather than to a conversation: a `file`
   link is accepted only when the issue links a workspace (its paths are
   checkout-relative and unresolvable otherwise); a `commit` must be one the
   issue links; a `worktree` must be a checkout of a workspace the issue links;
   `plan_stage`, `issue_stage`, `run`, `implementation` and `recovery` are
   **refused outright** — they address the retired plan flow, which the tracker
   does not extend.

An agent-supplied ref that fails either half refuses the whole
`issues.comment` / `comment_issue` call by name. Nothing partially lands.

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

`issues.columns {project_id}` answers the list. It takes a project so the verb
does not have to change when columns become per-project.

A closed issue keeps its status. Closing does not move it to `done` and moving
it to `done` does not close it: one is "where is this on the board", the other
is "is this still open", and the SPA shows both.

## The verbs

All under `issues.*`, registered in `api/v1` as a family of their own
(`bridge/src/api/v1/issues.rs`) with typed params and results and a fixture
under `fixtures/api/v1/` per verb.

**A verb's `since` is the minor it actually shipped in, and a fixture must not
claim one the bridge never served.** The ten the tracker shipped with say
`1.3.0`. `issues.track`, `issues.untrack` and `issues.for_agent` arrive later
and say the minor their own bump lands on — `versions.json` and `API_VERSION`
move together, and the new fixtures' `since` equals that number.

| Verb | Params | Result |
| --- | --- | --- |
| `issues.list` | `{project_id, state?, status?, assignee?, label?}` | `{issues: [Issue]}` |
| `issues.get` | `{issue_id}` | `{issue, timeline: [TimelineEntry]}` |
| `issues.create` | `{project_id, title, body?, status?, labels?, priority?, assignee?, links?}` | `{issue, dispatch}` |
| `issues.update` | `{issue_id, title?, body?, labels?, priority?, status?, state?}` | `{issue}` |
| `issues.assign` | `{issue_id, assignee, note?}` | `{issue, dispatch}` |
| `issues.comment` | `{issue_id, body, refs?}` | `{issue, comment}` |
| `issues.link` | `{issue_id, workspace_id?, branch?, commit?, conversation_id?, parent_issue_id?}` | `{issue}` |
| `issues.close` | `{issue_id, reason?}` | `{issue}` |
| `issues.reopen` | `{issue_id}` | `{issue}` |
| `issues.track` | `{issue_id, agent_id}` | `{issue}` |
| `issues.untrack` | `{issue_id, agent_id}` | `{issue}` |
| `issues.for_agent` | `{agent_id}` | `{agent_id, assigned: [IssueDigest], tracking: [IssueDigest]}` |
| `issues.columns` | `{project_id}` | `{project_id, columns: [{id, name}]}` |

Notes on each:

- **`issues.list`** answers newest first (`number` descending). Every filter is
  optional and they are ANDed. `state` absent means both; `assignee` takes the
  actor shape, plus the two words `"none"` (unassigned) and `"any"`. `state` and
  `status` are answered in SQL off the hoisted columns; `assignee` and `label`
  live inside the record and are applied to what that read answers. All four are
  params of the verb either way — a client sends them rather than filtering what
  it was given.
- **`issues.get`** answers the issue and its whole timeline. Comments and events
  interleave into one ascending list ordered by `(created_at|at, id)` — ids are
  time-ordered, so equal timestamps still have one stable order. An entry is the
  record itself with one more key naming which it is — the **spread** form, not a
  nested one:

  ```json
  {"type":"comment","id":"ic-…","issue_id":"issue-…","author":{"kind":"agent","agent_id":"agent-1"},
   "body":"starting on this","refs":[],"created_at":"2026-08-21T10:01:00Z"}
  {"type":"event","id":"ie-…","issue_id":"issue-…","at":"2026-08-21T10:01:00Z",
   "actor":{"kind":"user"},"kind":"moved","payload":{"from":"backlog","to":"in_progress"}}
  ```

  A comment stamps `created_at` and an event stamps `at`: one was written, the
  other happened. An event's `kind` is the event kind and has nothing to do with
  the `kind` discriminator inside an actor or an assignee.
- **`issues.create`** mints the number inside the same transaction as the insert,
  writes a `created` event, and — when it was given an `assignee` — runs the
  whole of `issues.assign` before answering. So `dispatch` on the result is the
  same shape `issues.assign` answers, and `null` when nothing was dispatched.
  The assignee is read BEFORE the issue is written: one this bridge cannot make
  sense of refuses the whole call rather than leaving a filed issue nobody asked
  for. `note` is accepted and delivered; a form need not offer one, because on a
  create the body IS the issue.

  A client should check `issue.assignee` on the answer when it sent one. The v1
  facade parses params into the typed struct and hands the implementation the
  struct serialised BACK, which drops any field this bridge does not know — so
  a bridge older than this verb's `assignee` files the issue unassigned and
  answers `ok`, with nothing in the refusal to say the choice went nowhere. That
  is true of every optional param on every v1 verb; it matters here because
  assignment is dispatch, and a dropped assignee means somebody walks away
  believing an agent is on it.
- **`issues.update`** applies only the fields present. Each one that actually
  changes something writes its own event: `moved` for `status`, `labelled` for
  `labels`, `closed`/`reopened` for `state`. A `title`/`body`/`priority` change
  writes no event — the record's `updated_at` is the whole history those need.
- **`issues.link`** takes one or more of its five keys and appends each,
  writing one `linked` event per link that was not already there.
- **`issues.close`** on a closed issue and **`issues.reopen`** on an open one are
  `conflict`, not silent no-ops: the caller believed something that was not true.

Errors are `not_found`, `invalid_params`, `conflict` and `internal`, named the
way the workspace family names them.

`fixtures/api/versions.json` goes to `1.2.0` — a minor bump, because every verb
here is new and nothing existing changed shape. `supported_majors` is untouched.

## Assignment is dispatch

`issues.assign` takes one tagged `assignee`. Assignment IS dispatch here, so a
single field says both who holds the issue and where the work runs; there is no
second `dispatch` object for the two to disagree in. The five kinds:

| `assignee` | What happens | Stored assignee |
| --- | --- | --- |
| `{"kind":"user"}` | Nothing is dispatched. | `{"kind":"user"}` |
| `{"kind":"project_agent"}` | `project.ensure_conversation` on the issue's project, an agent on it if it has none, then deliver. | `{"kind":"project_agent"}` |
| `{"kind":"agent","agent_id":…}` | Deliver into that agent's conversation. The agent must be on this issue's project. | `{"kind":"agent","agent_id":…}` |
| `{"kind":"new_workspace","name"?,"isolation"?,"provider"?,"model"?,"effort"?}` | `workspace.create` in the issue's project, `agent.add` on the workspace's conversation owner, deliver. | `{"kind":"agent","agent_id":…}` — the agent that was made |
| `{"kind":"new_agent","workspace_id","provider"?,"model"?,"effort"?}` | `agent.add` on that workspace's conversation owner, deliver. | `{"kind":"agent","agent_id":…}` |

`new_workspace` defaults its `name` to the issue's title and its `isolation` to
the project's own setting — it passes no `isolation` at all when none was asked
for, which is how `workspace.create` reads "the project's".

**The agent's choice is spelled `provider` on the wire and `harness` in a
tool**, which is the rule this codebase already follows rather than a new one:
`agent.add` takes `provider`, and the project surface's `add_workspace_agent`
takes `harness` and maps it (`AgentChoiceArgs`, `app/projects/agent_writes.rs`,
which says so in as many words — the model is told what it is choosing between,
the daemon is told which field it is). So `issues.assign` takes `provider` and
the SPA passes its `agentChoiceParams` straight through, while `assign_issue`
takes `harness` and the bridge maps it before `agent.add`. Neither surface
accepts the other's spelling: one word per surface, decided here, is what keeps
the two from drifting into both.

Absent is absent, not null. A choice key that was not asked for is left OUT of
the params `agent.add` is called with, because that verb reads the PRESENCE of a
key to tell "run it on this" from "run it on whatever the workspace runs on".

An optional `note` on `issues.assign` is extra instruction text delivered under
the issue. It is not stored on the issue: the issue's body is the issue, and a
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

So `issues.assign` with `new_workspace` **defers**, exactly the way
`workspace.create` does: it hands the cut to the drain with the rest of the
dispatch hung off the end of it, and the client's reply is the one the drain
publishes once the agent has been made and the issue handed over. The reply
shape is unchanged — `{issue, dispatch}` with every id filled in — because the
client never sees the placeholder. The other four kinds do not defer; they are
a SQLite write and an in-memory lookup.

Nothing is written to the issue until the checkout exists. A cut that fails
leaves the issue exactly as it was: unassigned, in its old column, with no
`assigned` or `dispatched` event. An issue must never name an agent that was
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
in this issue's project is refused in the same words
(`workspace … is not in project …`, `agent … is not in project …`).

### The dispatch result and the events

A dispatching assign answers

```json
{
  "issue": { … },
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
something. Handing an issue to an agent that already exists links the workspace
that agent is working in, looked up from its conversation, because an issue that
recorded only the conversation would name who is on it and not where the code
is — which is the question anybody reading the issue later asks. The project's
agent links no workspace: it works in the repository itself. **Unassigning
unlinks nothing.** Where an issue was worked is a fact about its history, and
handing it back does not unmake the checkout.

Which is what makes self-assignment worth asking an agent for, and the
issue-tools prompt does: an agent that picks up an issue nobody handed it
assigns it to itself, and the checkout is recorded by that alone. `link_issue`
is then left with what assignment cannot know — the branch cut for the issue,
each commit that lands for it, and any second workspace.

A dispatching assign also **moves the issue to `in_progress`** and writes a
`moved` event, when the issue is open and its status is `backlog` or `ready`.
An issue already at `in_progress`, `in_review` or `done` is left where it is:
the board position was set deliberately and a reassignment is not a reason to
rewind it.

### The envelope

The delivered message is an ordinary conversation message on the user's side of
the conversation — that is the side an instruction arrives on whoever wrote it —
carrying an **issue envelope** the way a hand-off carries `from_agent`:

```json
{
  "id": "message-9",
  "role": "user",
  "from_issue": {
    "issue_id": "issue-01K5Z…",
    "number": 12,
    "title": "Kanban drag does not persist",
    "body": "Dragging a card to In review …",
    "links": { "workspace_ids": [], "branches": [], "commits": [],
               "conversation_ids": [], "parent_issue_id": null }
  },
  "from_agent": { "id": "agent-01K5Y…" },
  "body": "…"
}
```

`from_issue` is optional and absent on every other message, so a client that has
never heard of it reads the conversation exactly as it always has. `from_agent`
is present when an agent did the assigning and absent when the user did.

The message body is the issue rendered as prose — `#12 <title>`, then the body,
then the note if one was given — so a harness that never learns about
`from_issue` still receives the whole issue. The envelope is for the SPA, which
draws the message as an issue card and links `#12`.

The delivery prompt the harness actually reads gains one line, beside the
existing "These messages came from agent `…`" and "The user sent this from
workspace `…`" lines (`bridge/src/operation.rs`):

> This message hands you issue #12 "Kanban drag does not persist"
> (`issue-01K5Z…`). Comment your progress on it with `comment_issue`, and move
> it to In review with `move_issue` when you report Complete.

## The MCP tools

Eight tools, on **both** the coding surface and the project surface:

`list_issues`, `get_issue`, `create_issue`, `comment_issue`, `assign_issue`,
`move_issue`, `close_issue`, `link_issue`, `track_issue`, `untrack_issue`.

Each is a thin wrapper over the verb of the same shape. The bridge knows who is
calling, so:

- **no tool takes a project**. The scope is the project the calling agent's
  conversation owner is bound to — a coding agent's workspace's project, a
  project agent's own. A call carrying a project id is parsed as though it had
  not, exactly as the project surface's tools already are.
- **no tool takes an author or an actor**. The calling agent is the author of
  every comment it writes and the actor of every event it causes.
- an issue in another project is `unknown issue_id`, and an assignee in another
  project is refused by name. The gate is on the socket (`allowed_on`) as well
  as in the tool list a session is shown, so a harness writing its own frames
  reaches no further than one that reads the list.

`track_issue` and `untrack_issue` take an `issue_id` and nothing else. Which
agent is tracking is the caller — a tool cannot subscribe somebody else, the
way it cannot sign a comment as somebody else — so there is no `agent_id` for a
call to carry and none for it to get wrong.

`assign_issue` takes the same five assignee kinds the verb does, so an agent can
hand work to a named agent, to the project's agent, back to the user, or to a
workspace and agent it asks Build to create. On the two creating kinds it spells
the agent's choice `harness`, the way every other tool on these surfaces does,
where the wire verb spells it `provider` — see
[Assignment is dispatch](#assignment-is-dispatch).

`move_issue {issue_id, status}` is `issues.update` narrowed to one field,
because moving a card is what an agent does and offering it the whole update
would invite it to rewrite a title it was not asked about.

### The prompt note

One shared block appended to every template that carries these tools, the way
`MESSAGE_AGENT_NOTE` is appended (`bridge/src/templates.rs`) — so the wording
cannot drift between surfaces and a project that overrides one template
overrides only that one. It says:

- when a message hands you an issue, that issue is the work: comment your
  progress on it rather than only reporting at the end;
- move it to **In review** when you report Complete — you are saying the work is
  ready to be looked at, not that it is accepted;
- hand work off by **assigning** the issue, not by messaging: an assignment
  delivers the issue and starts the agent, and leaves a record on the issue that
  a message does not;
- file an issue for follow-up work you find and do not do. An issue is cheap and
  the thing you noticed is otherwise only in your conversation.

## Tracking

An issue that several agents are working around is only useful if they hear
about it. Tracking is how: an agent says it wants to know, and every later
change to that issue is delivered into its conversation as a message. No
polling, and no staying awake — a delivered notice starts the agent's turn like
any other delivered message, so an idle agent wakes to it.

### Who is tracking

`trackers` is a list of agent ids on the issue, ordered by when each started,
deduped, capped at 50. An issue nobody watches answers `[]` rather than
omitting the key.

An agent joins the list three ways:

- **It asks**, with `issues.track` / the `track_issue` tool.
- **It is assigned the issue.** Assignment is dispatch, so the agent that gets
  the work is the agent that most needs to hear about it; adding it is not a
  courtesy but the thing that makes the hand-off two-way. The `tracked` event
  for this carries `{by: "assignment"}`, so a timeline reader can tell a
  request from a consequence.
- Nothing else. Commenting on an issue does not subscribe you to it: an agent
  that answers a question and moves on should not be woken for the next month.

An agent leaves only by asking (`issues.untrack` / `untrack_issue`). **Being
unassigned does not untrack**, which is deliberate: handing work on is exactly
when the previous holder still wants to know how it went, and an agent that
does not can say so in one call.

Tracking an issue that is already tracked, and untracking one that is not, both
answer the issue unchanged and write no event. This is a set, and saying a
thing twice is not a second fact.

### What a change delivers

Every change to a tracked issue delivers one notice per tracker: a status or
column move, a state change, a title, body, labels or priority edit, a new
comment, an assignment, a link. The notice is an ordinary conversation message
on the user's side — the side an instruction arrives on whoever wrote it —
carrying:

- **`from_build: true`**, the mark the restart notice already uses. It says the
  daemon wrote this and nobody is waiting on an answer to it. A client draws it
  as Build's own words rather than as the reader's.
- **`from_issue`** — the same envelope a dispatched issue carries, narrowed to
  what a notice needs: `{issue_id, number, title}`. It is what lets a client
  draw the notice as a card on the issue and link `#13`.
- **A one-line body** saying what changed and who changed it: `#13 moved to In
  review by agent-01K5Z…`. When the change is a comment, the comment's body
  follows on its own line, because the whole point of hearing about a comment
  is reading it.

**An agent is never told about its own change.** The actor is excluded from the
delivery, always. An agent that moves a card and is then woken to be told it
moved a card would answer its own message, and two agents each tracking the
other's issue would do it forever.

A change that alters nothing delivers nothing, for the same reason it writes no
event: moving a card to the column it is already in is not news.

Delivery is durable and goes down the path every other delivered message goes
down — the same queue, the same receipt, the same start. It skips
`note_user_message`, as an agent-to-agent hand-off does: Build telling an agent
something is the work happening, not somebody speaking to the human, and it
must not move the inbox anchor under a reader.

### An agent says what it did

Separately from tracking, and for a different reader: **every issue write an
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
  "body": "Commented on #13 Issue tracking: an agent that tracks an issue …",
  "issue_action": {
    "action": "commented_on",
    "issue_id": "issue-01K5Z…",
    "number": 13,
    "title": "Issue tracking: an agent that tracks an issue …",
    "comment_id": "ic-01K5Z…"
  }
}
```

`action` is one of `created`, `assigned`, `moved`, `closed`, `reopened`,
`commented_on`, `linked`, `updated` — a slug, the way a column is, so a client
renders the label and the bridge does not decide the wording twice. `comment_id`
is present only on `commented_on`, and is what lets a client deep-link the
comment rather than the issue.

**One message per write, never two.** A call that relabels and moves is one
thing the agent did, and two messages about it would be two lines in a
conversation for one action. This is why the message is posted in
`commit_issue_write` — the one funnel every tracker write already goes through —
rather than in each verb.

A create is the one write that does not pass through that funnel, because its
number is minted inside the insert's own transaction; the create path calls the
same hook by hand, and the rule is unchanged. There is no
`created_and_assigned`: a create and an assignment are two separate writes, and
`create_issue` takes no assignee at all, so no agent can reach the pair in one
call.

**Only when the actor is an agent with a conversation.** A human moving a card
on the board is already looking at the board; posting into a conversation nobody
is reading to tell them what they just did on screen is noise. So the api path
posts nothing, and the check is on the actor rather than on which verb was
called — the same verb serves both.

The message is in the ACTING agent's conversation and nobody else's. An
assignment posts "Assigned #13 to …" for the agent that assigned; what the
assignee gets is the dispatched issue itself, which it was already getting.

Its shape is pinned in `fixtures/api/v1/thread.page.json` with the notice, for
the same reason and in the same file.

### The Complete reminder

An agent reports Complete and walks away from three open issues assigned to it.
Nobody is told, the issues sit in In progress, and whoever assigned them finds
out by going to look.

So on **Complete**, and only on Complete, Build delivers one more `from_build`
message into that agent's own conversation, naming every open issue assigned to
it whose column is not Done, and saying the three ways out: finish it, comment
where it got to, or hand it back with a comment saying why.

- **Not on Blocked or Waiting.** Both are the agent saying it cannot finish,
  which is already an answer about the work. A list of what it has not finished
  would be telling it what it just told us.
- **Not when it holds nothing open.** Silence is the right answer there.
- **Done is excluded**, not just closed. An issue parked in Done is one the
  agent is finished with even if nobody has closed it, and a reminder that
  includes those is noise — which is a reminder an agent learns to answer
  without reading.
- **It names every issue**, with the column each is in, rather than counting
  them. "You still hold 3 issues" makes the agent go and look, and the looking
  is the part Build can do.
- **It repeats.** An agent that answers with another Complete while still
  holding the same issues is reminded again. That is the point rather than a
  bug to suppress: the way out is one tool call.

It runs after the report's own automatic activity, so an issue the same
Complete moved to In review is described as it now stands.

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

`issues.for_agent {agent_id}` answers what one agent is on:

```json
{
  "agent_id": "agent-01K5Z…",
  "assigned": [{ "issue_id": "issue-01K5Z…", "number": 13, "title": "…",
                 "state": "open", "status": "in_progress",
                 "updated_at": "2026-09-20T15:04:00Z" }],
  "tracking": [ … the same digest … ]
}
```

A digest and not the whole issue: this is a list somebody scans, and the body
of thirty issues is not a list. An issue the agent both holds and tracks — which
is every assigned one — appears in both, because the two questions are
different and a client showing one should not have to know about the other.

Both lists are newest-updated first. Scoped to the agent's own project, which
the bridge resolves from the agent rather than taking as an argument.

### Scope

An agent may track only issues of its own project, and may track only itself:
`issues.track` names an `agent_id`, and a tool call's is forced to the caller.
The wire verb is the user's, so it may name any agent of the issue's project
and refuses one outside it by name — the same refusal every project-scoped
handler gives.

## Push

Issues push over the existing changes subscription
(`bridge/src/changes.rs`) as a new `Kind::Issues`, wire-spelled `"issues"`,
scoped to the **project** entity:

```json
{ "subscription_id": "s-inbox",
  "scope": { "kind": "entity", "id": "proj-1" },
  "kinds": ["issues"],
  "mode": "realtime" }
```

An item names the issues that moved:

```json
{ "entity_id": "proj-1", "issues": { "issue_ids": ["issue-01K5Z…"], "truncated": false } }
```

Content-free beyond the ids, like every other signal Build sends about work it
cannot read: a client refetches `issues.list` or `issues.get`. Past 200 ids in
one un-flushed window the item is `truncated` and the ids are dropped, which
means "refetch the list", exactly as a `files` item's truncation does.

`issues` is not a worktree kind: it is not paced by the settle window, it needs
no filesystem watcher, and it never makes a subscription answer `polled`.

A subscription scoped `{"kind":"all"}` that names `issues` receives every
project's items, each with `entity_id` set to that project's `proj-N` id — `all`
covers whatever is noted, and nothing about the project-scoped spelling above is
a restriction. Under `all` an `issues` item is therefore the one item whose
`entity_id` is a project rather than a work entity.

An `issues` note fires the **subscription path only**. It emits no legacy
`entity.changed` and does not bump the board revision: a client in legacy mode
has no issues surface to refetch, and bumping the board on every comment would
repaint the feed for something the feed does not show.

Every mutation — create, update, assign, comment, link, move, close, reopen, and
each automatic activity below — notes its project once, after the write lands.
A `changes` subscriber also keeps hearing `thread` on the conversation an issue
was delivered into: that is the same message arriving, seen from the other side.

## Automatic activity

Two things happen without anyone asking.

**An agent that holds a dispatched issue reports Complete.** Its report's body
is added to the issue as a comment authored by that agent, and the issue moves
to `in_review` with a `moved` event whose payload says `{"by": "report"}`. It
holds the issue when the issue's `assignee` is `{"kind":"agent"}` naming that
agent, or the issue's `links.conversation_ids` holds that agent's conversation
owner and nobody else holds it. A Blocked or Failed report adds the comment and
does **not** move the issue: blocked is not ready to look at. One report
comments on at most one issue — the one most recently dispatched to that agent —
so an agent that has held three issues does not write the same report on all of
them.

**A workspace linked to an issue is finished.** `workspace.finish` closes every
open issue that links that workspace, with a `closed` event whose payload is
`{"reason": "workspace_finished", "workspace_id": …}`. It happens when Done is
accepted — after the eligibility measure that proves every commit is already in
the remote it pushes to — rather than after the folder is gone: eligibility is
what proves the work is somewhere else, and a removal that later fails on disk
does not make the work un-done.

Neither of these moves the inbox anchor or crosses a dismissal line. They are
the work happening, not somebody speaking to the human.

## Storage

Three tables, added to `bridge/src/store/schema.rs`, taking `SCHEMA_VERSION`
from 7 to 8. They are all `CREATE TABLE IF NOT EXISTS`, so a store written by an
older bridge gains them on the next open with no `ALTER` and no backfill.

```sql
CREATE TABLE IF NOT EXISTS tracker_issues (
    id          TEXT PRIMARY KEY,
    project_key TEXT NOT NULL,
    number      INTEGER NOT NULL,
    state       TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    record      TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS tracker_issues_number
    ON tracker_issues(project_key, number);
CREATE INDEX IF NOT EXISTS tracker_issues_by_project
    ON tracker_issues(project_key, number DESC);

CREATE TABLE IF NOT EXISTS tracker_comments (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT NOT NULL,
    created_at TEXT NOT NULL,
    record     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tracker_comments_by_issue
    ON tracker_comments(issue_id, created_at, id);

CREATE TABLE IF NOT EXISTS tracker_events (
    id       TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL,
    at       TEXT NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tracker_events_by_issue
    ON tracker_events(issue_id, at, id);
```

The design rules the rest of the store follows, applied here:

- **A record is a record.** An issue, a comment and an event are each small,
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
- **Nothing is deleted in phase 1.** An issue is closed, not removed, so a
  number is never reused and a timeline never loses an entry. Deleting a project
  deletes its issues, the way it deletes its plans and runs.

### A different backing store later

The store surface is six methods, all of them whole-record:

| Method | What it is for |
| --- | --- |
| `create_tracker_issue(draft, events)` | Mints the number inside the insert's own transaction and answers the issue as stored. |
| `save_tracker_issue_activity(issue, comments, events)` | The record moved and the timeline says why, in one transaction. Empty slices are a plain save. |
| `load_tracker_issue(issue_id)` | One issue, or `None`. |
| `list_tracker_issues(project_path, filter)` | One project's, newest first, narrowed by state and status. |
| `load_tracker_timeline(issue_id)` | Comments and events merged into one ascending list. |
| `delete_tracker_issues_of_project(project_path)` | Only reached by project deletion. |

None of them takes SQL, a connection, or a row. Replacing SQLite with something
else — a service, a file per issue, a git-backed store — is implementing those
six against something else; nothing above this line knows what is underneath it.

An append is idempotent by id (`INSERT OR IGNORE`), so a retry of a write whose
answer was lost adds nothing a second time.

## Scope rules, in one place

1. An issue belongs to exactly one project and never moves between projects.
2. An agent reaches only its own project's issues. Which project that is comes
   from its conversation owner's binding, never from a tool argument.
3. An assignee must be of the issue's project: an agent on one of its
   workspaces, its project agent, or the user.
4. A link must be of the issue's project: a workspace of it, a conversation
   owner bound to it, a parent issue in it.
5. A comment's typed refs are fenced twice — shape, then ownership by the issue.
6. The user, over the wire, may act on any project's issues; `project_id` is a
   param there because the browser is not scoped to a project the way an agent
   is.

## What the SPA phase builds

Four surfaces, all under the project (`#/project/<project_id>`), which is
already where a project's workspaces live.

**The list.** Rows of `#<number>  title`, with the assignee, the labels, the
priority and the status column on each. Filters for state, status, assignee and
label, matching `issues.list`'s params one for one so a filter is a param and
not a client-side pass over everything. A new-issue composer: title, markdown
body, and the assignee picker.

**The kanban.** The same issues laid out in columns from `issues.columns`, in
that order, each card the compact form of a row. Dragging a card between columns
is `issues.update {status}`; the card moves optimistically and the column
repaints from the push. A column is not a filter the user typed — it is what
`status` says — so an empty column is still drawn.

**The issue page.** The title, the markdown body, the state and status, the
labels, the priority, the assignee, the links, and the timeline: comments and
events interleaved, ascending, each stamped with its actor and time. A comment
composer at the bottom. A delivered issue's conversation is one press away
through `links.conversation_ids`.

**The assignee picker.** One control over all five kinds — the user, the
project's agent, any agent on any workspace of the project (grouped by
workspace, from `workspace.list` plus `agent.list`), a new agent on an existing
workspace, or a new workspace. Choosing one of the last two is where the
harness/model/effort selects appear. The control says what it is about to do —
"cut a workspace and start an agent on it" — because assigning starts work and
the user should not discover that afterwards.

**The agent's activity entry.** Per agent, what it holds and what it watches,
from `issues.for_agent {agent_id}` — two digest lists, each newest-updated
first, each entry `{issue_id, number, title, state, status, updated_at}`. An
assigned issue appears in both lists; that is not a bug to de-duplicate, it is
the two questions being different.

**Push.** The project page subscribes `{scope: {kind: "entity", id: project_id},
kinds: ["issues"]}` and refetches on an item. An issue page open on an issue
named in an item refetches that issue.

**On a message.** A conversation message carrying `from_issue` draws as an issue
card — `#12`, the title, a fold for the body — linking the issue page. It is
still a message and still reads in sequence; the card is how it is drawn, not a
separate kind of thing.

## Boundaries

Issues do not replace the plan flow's documents and they do not become one. They
hold no stages, no approvals, no diff and no review state. An issue says what
should be done and where the doing is happening; the doing itself is a
workspace, a conversation and a diff, each of which already has a surface.

Assigning starts an agent. Nothing else here does: closing an issue stops
nothing, moving a card stops nothing, and unassigning stops nothing. Stopping an
agent is `agent.remove` and `workspace.delete`, which say what they take with
them.
