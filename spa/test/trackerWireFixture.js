// The tracker's wire, as the Issues Spec writes it (planning/v2/Issues Spec.md).
//
// Every suite that scripts `issues.*` answers builds them here, so the shapes
// this client is written against live in one file and a change to the wire is
// one edit rather than a search. The bridge phase is being built beside this
// one; until its verbs land, this fixture IS the contract the SPA is tested to.

/** The five fixed columns of phase 1, in order — what `issues.columns` answers. */
export const columns = () => [
  { id: "backlog", name: "Backlog" },
  { id: "ready", name: "Ready" },
  { id: "in_progress", name: "In progress" },
  { id: "in_review", name: "In review" },
  { id: "done", name: "Done" },
];

/** An issue record. Every field the spec's table names is present, because a
 *  bridge answers the whole record and a client that only ever sees partial
 *  ones learns to tolerate what it should not. */
export const issue = (over = {}) => ({
  id: `issue-01K5Z${over.number || 1}`,
  project_id: "p1",
  number: 1,
  title: "Kanban drag does not persist",
  body: "Dragging a card to In review leaves it where it was after a reload.",
  state: "open",
  status: "backlog",
  labels: [],
  priority: "none",
  assignee: null,
  links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_issue_id: null },
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
  id: "ic-01K5Z2",
  issue_id: "issue-01K5Z1",
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
  id: "ie-01K5Z1",
  issue_id: "issue-01K5Z1",
  at: "2026-08-21T10:00:00Z",
  actor: { kind: "user" },
  kind: "created",
  payload: {},
  ...over,
});

/** What `issues.get` answers: the issue, and its whole timeline ascending. */
export const issueDetail = (over = {}, timeline = [event()]) => ({ issue: issue(over), timeline });

/** What a dispatching `issues.assign` answers beside the issue. */
export const dispatch = (over = {}) => ({
  kind: "new_workspace",
  workspace_id: "ws-3f2a91c4",
  entity_id: "run-5d90b1e7",
  agent_id: "agent-01K5Z9",
  operation_id: "op-1",
  ...over,
});
