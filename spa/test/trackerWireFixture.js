// The tracker's wire, as the Tasks Spec writes it (planning/v2/Tasks Spec.md).
//
// Every suite that scripts `tasks.*` answers builds them here, so the shapes
// this client is written against live in one file and a change to the wire is
// one edit rather than a search. The bridge phase is being built beside this
// one; until its verbs land, this fixture IS the contract the SPA is tested to.

/** The five fixed columns of phase 1, in order — what `tasks.columns` answers. */
export const columns = () => [
  { id: "backlog", name: "Backlog" },
  { id: "ready", name: "Ready" },
  { id: "in_progress", name: "In progress" },
  { id: "in_review", name: "In review" },
  { id: "done", name: "Done" },
];

/** A task record. Every field the spec's table names is present, because a
 *  bridge answers the whole record and a client that only ever sees partial
 *  ones learns to tolerate what it should not. */
export const task = (over = {}) => ({
  id: `task-01K5Z${over.number || 1}`,
  project_id: "p1",
  number: 1,
  title: "Kanban drag does not persist",
  body: "Dragging a card to In review leaves it where it was after a reload.",
  state: "open",
  status: "backlog",
  labels: [],
  priority: "none",
  assignee: null,
  links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
  created_by: { kind: "user" },
  created_at: "2026-08-21T10:00:00Z",
  updated_at: "2026-08-21T10:00:00Z",
  closed_at: null,
  ...over,
});

/** A timeline comment. The record's own fields sit on the entry beside `type`
 *  — the wire is internally tagged — and a comment stamps `created_at`. */
export const comment = (over = {}) => ({
  type: "comment",
  id: "tc-01K5Z2",
  task_id: "task-01K5Z1",
  author: { kind: "user" },
  body: "This reproduces on a phone too.",
  refs: [],
  created_at: "2026-08-21T10:01:00Z",
  ...over,
});

/** A timeline event. An event stamps `at`, and its `kind` is the event kind —
 *  unrelated to the `kind` inside an actor. */
export const event = (over = {}) => ({
  type: "event",
  id: "te-01K5Z1",
  task_id: "task-01K5Z1",
  at: "2026-08-21T10:00:00Z",
  actor: { kind: "user" },
  kind: "created",
  payload: {},
  ...over,
});

/** What `tasks.get` answers: the task, and its whole timeline ascending. */
export const taskDetail = (over = {}, timeline = [event()]) => ({ task: task(over), timeline });

/** What a dispatching `tasks.assign` answers beside the task. */
export const dispatch = (over = {}) => ({
  kind: "new_workspace",
  workspace_id: "ws-3f2a91c4",
  entity_id: "run-5d90b1e7",
  agent_id: "agent-01K5Z9",
  operation_id: "op-1",
  ...over,
});
