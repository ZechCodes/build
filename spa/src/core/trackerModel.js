// The issue tracker's record vocabulary: the columns, the priorities, and the
// one tagged actor shape that names a person or an agent
// (planning/v2/Issues Spec.md).
//
// The tracker is not the retired plan flow. That flow owns the singular
// `issue.*` verbs and core/issueModel.js; this owns the plural `issues.*` and
// every module named `tracker*`. Nothing here extends that flow, and the two
// never share a record.
//
// No DOM, no app imports: every renderer and every view reads its labels here,
// so the same issue reads the same way on a row, on a card and on its page.

/** The five columns of phase 1, in order — what a board draws before
 *  `issues.columns` has answered, and what it falls back to for a bridge that
 *  does not serve the verb. `status` is a slug string rather than an enum
 *  precisely so a later per-project column set is a record change, so nothing
 *  here is allowed to be the only list of columns in the client. */
export const FALLBACK_COLUMNS = Object.freeze([
  Object.freeze({ id: "backlog", name: "Backlog" }),
  Object.freeze({ id: "ready", name: "Ready" }),
  Object.freeze({ id: "in_progress", name: "In progress" }),
  Object.freeze({ id: "in_review", name: "In review" }),
  Object.freeze({ id: "done", name: "Done" }),
]);

/** The columns to draw: what the bridge answered, normalized to `{id, name}`,
 *  or the five above when it has answered nothing yet. A column the bridge
 *  named without a display name is drawn under its slug — an unnamed column is
 *  still a column, and dropping it would lose the issues standing in it. */
export function columnsOf(columns) {
  const listed = (columns || [])
    .map((column) => ({ id: String(column?.id || ""), name: String(column?.name || column?.id || "") }))
    .filter((column) => column.id);
  return listed.length ? listed : FALLBACK_COLUMNS.map((column) => ({ ...column }));
}

/** What to call the column a status names. An issue standing in a column the
 *  bridge no longer offers still says where it is: the slug is shown rather
 *  than nothing, because "somewhere this client cannot name" is worse than the
 *  name the record carries. */
export function columnName(columns, status) {
  const slug = String(status || "");
  return columnsOf(columns).find((column) => column.id === slug)?.name || slug;
}

/** The priorities, lowest first. `none` is a value and not an absence, so it is
 *  on the list and is the default. */
export const PRIORITIES = Object.freeze([
  Object.freeze({ id: "none", label: "None" }),
  Object.freeze({ id: "low", label: "Low" }),
  Object.freeze({ id: "medium", label: "Medium" }),
  Object.freeze({ id: "high", label: "High" }),
  Object.freeze({ id: "urgent", label: "Urgent" }),
]);

export const priorityLabel = (priority) =>
  PRIORITIES.find((candidate) => candidate.id === priority)?.label || PRIORITIES[0].label;

/** Whether a priority is worth a mark of its own. `none` and `low` are the
 *  quiet ones: a board where every card wears a chip says nothing with one. */
export const priorityIsMarked = (priority) => priority === "medium" || priority === "high" || priority === "urgent";

/** Open or closed, which is independent of the Done column: one says whether
 *  the work is still live, the other where it stands on the board, and the SPA
 *  shows both. */
export const stateLabel = (state) => (state === "closed" ? "Closed" : "Open");

// ---- actors ---------------------------------------------------------------
//
// One tagged shape everywhere a person or an agent is named — an assignee, a
// comment's author, an event's actor. `project_agent` is only ever an
// assignee; an author and an actor are always `user` or `agent`.

/** The key an unassigned issue stands under. The wire's own word for it —
 *  `issues.list` takes `"none"` beside the actor shapes — so the filter bar's
 *  value and the param it becomes are the same string. */
export const UNASSIGNED = "none";

/**
 * The key one actor is the same actor under.
 *
 * Every control that compares actors — the filter bar's selection, the picker's
 * tick, "is this already the assignee" — compares these rather than the objects,
 * because two reads of the same agent are two objects. `null` (unassigned) has
 * a key of its own so it can be selected like any other value.
 */
export function assigneeKey(assignee) {
  if (!assignee || !assignee.kind) return UNASSIGNED;
  if (assignee.kind === "agent") return `agent:${assignee.agent_id || ""}`;
  return String(assignee.kind);
}

export const sameAssignee = (left, right) => assigneeKey(left) === assigneeKey(right);

/** The actor a key names, back in the tagged shape the wire takes. The inverse
 *  of `assigneeKey` for the three kinds a control offers directly; the two
 *  creating kinds are built by core/trackerAssignee.js, which has the workspace
 *  and the agent choice to build them from. */
export function assigneeFromKey(key) {
  const token = String(key || "");
  if (token === "user" || token === "project_agent") return { kind: token };
  if (token.startsWith("agent:")) return { kind: "agent", agent_id: token.slice("agent:".length) };
  return null;
}

/** How much of an agent's id a label can hold when nothing else names it. The
 *  same four characters a message's sender chip wears (core/thread.js), so one
 *  agent reads as one agent wherever it appears. */
const AGENT_LABEL_CHARS = 4;

const shortAgentLabel = (agentId) => {
  const trimmed = String(agentId || "").trim();
  const body = trimmed.includes("-") ? trimmed.slice(trimmed.indexOf("-") + 1) : trimmed;
  const short = body.replace(/[^a-z0-9]/gi, "").slice(0, AGENT_LABEL_CHARS).toUpperCase();
  return short ? `Agent ${short}` : "Agent";
};

/**
 * What to call an actor.
 *
 * `labels` is what the caller knows about the agents of this project — the
 * picker builds it from the workspaces and their agent digests
 * (core/trackerAssignee.js), so an agent is called what the picker calls it.
 * An agent nobody can name is called by four characters of its id rather than
 * by nothing: a timeline entry with no actor on it reads as an accident.
 */
export function actorLabel(actor, labels = {}) {
  if (!actor || !actor.kind) return "Unassigned";
  if (actor.kind === "user") return "You";
  if (actor.kind === "project_agent") return "Project agent";
  if (actor.kind !== "agent") return String(actor.kind);
  return labels[actor.agent_id] || shortAgentLabel(actor.agent_id);
}

/** The initials an avatar wears for an actor: the human's, the project agent's,
 *  and an agent's own. Never more than two characters — it sits in a circle. */
export function actorInitials(actor) {
  if (!actor || !actor.kind) return "–";
  if (actor.kind === "user") return "Y";
  if (actor.kind === "project_agent") return "P";
  return "A";
}

/** An issue's links, with every list present. A record written by a bridge that
 *  left one out reads as empty rather than as undefined, so every rail row can
 *  be drawn without asking whether it has anything to draw. */
export function issueLinks(issue) {
  const links = issue?.links || {};
  return {
    workspace_ids: links.workspace_ids || [],
    branches: links.branches || [],
    commits: links.commits || [],
    conversation_ids: links.conversation_ids || [],
    parent_issue_id: links.parent_issue_id || null,
  };
}
