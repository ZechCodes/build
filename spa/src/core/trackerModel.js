// The task tracker's record vocabulary: the columns, the priorities, and the
// one tagged actor shape that names a person or an agent
// (planning/v2/Tasks Spec.md).
//
// The tracker is not the retired plan flow. That flow owns the singular
// `task.*` verbs and core/taskModel.js; this owns the plural `tasks.*` and
// every module named `tracker*`. Nothing here extends that flow, and the two
// never share a record.
//
// No DOM, no app imports: every renderer and every view reads its labels here,
// so the same task reads the same way on a row, on a card and on its page.

/** The five columns of phase 1, in order — what a board draws before
 *  `tasks.columns` has answered, and what it falls back to for a bridge that
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
 *  still a column, and dropping it would lose the tasks standing in it. */
export function columnsOf(columns) {
  const listed = (columns || [])
    .map((column) => ({ id: String(column?.id || ""), name: String(column?.name || column?.id || "") }))
    .filter((column) => column.id);
  return listed.length ? listed : FALLBACK_COLUMNS.map((column) => ({ ...column }));
}

/**
 * What each column means, for a reader who has never seen this board.
 *
 * Every sentence here is the Tasks Spec's own, because the board is the one
 * place the user meets rules the agents are told outright and the user is not:
 *
 *  - a dispatch moves a task to In progress from Backlog or Ready, and
 *    leaves it alone anywhere further along ("the board position was set
 *    deliberately and a reassignment is not a reason to rewind it");
 *  - an agent moves a card to In review when it reports Complete — "you are
 *    saying the work is ready to be looked at, not that it is accepted";
 *  - "closing does not move it to Done and moving it to Done does not close
 *    it: one is 'where is this on the board', the other is 'is this still
 *    open'".
 *
 * Nothing is invented for the columns the spec only names. Backlog and Ready
 * say what a dispatch does to them, which the spec does state, and no more.
 */
const COLUMN_NOTES = Object.freeze({
  backlog: "Filed, not started. Assigning it to an agent moves it to In progress.",
  ready: "Ready to pick up. Assigning it to an agent moves it to In progress.",
  in_progress: "An agent has been handed this and started on it.",
  in_review:
    "An agent moves a card here when it reports Complete: the work is ready to be looked at, not that it is accepted.",
  done: "The work is over. Closing is separate — a closed task keeps its column, and a card in Done can still be open.",
});

/** The sentence every column shares, under whatever its own says. A column is
 *  where a task stands, and that is a different question from whether it is
 *  still open — the one thing about this board that surprises people. */
export const COLUMN_NOTE_SHARED =
  "A column is where a task stands on the board; open or closed is whether it is still live. The two move independently.";

/**
 * What to say about one column on hover, or behind its info glyph.
 *
 * A column the bridge named but this build has no words for says the shared
 * sentence alone rather than nothing: a later per-project column set is a
 * record change, and a board of unexplained columns would be worse than one
 * with a general note on each.
 */
export function columnNote(columns, status) {
  const slug = String(status || "");
  const own = COLUMN_NOTES[slug];
  return own ? `${own} ${COLUMN_NOTE_SHARED}` : COLUMN_NOTE_SHARED;
}

/** What to call the column a status names. A task standing in a column the
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

/** Labels as a person types them — commas, because that is how anyone writes a
 *  short list. Trimmed, emptied out and deduped, which is what the verb does to
 *  them anyway; doing it here means the field shows what will be stored. */
export const labelsFromText = (text) => [
  ...new Set(String(text || "").split(",").map((label) => label.trim()).filter(Boolean)),
];

export const priorityLabel = (priority) =>
  PRIORITIES.find((candidate) => candidate.id === priority)?.label || PRIORITIES[0].label;

/** Whether a priority is worth a mark of its own. `none` and `low` are the
 *  quiet ones: a board where every card wears a chip says nothing with one. */
export const priorityIsMarked = (priority) => priority === "medium" || priority === "high" || priority === "urgent";

/** Whether a priority is pressing enough to say on a LIST ROW (#45). A quieter
 *  bar than the board card's: a row is read a dozen at a time down a column,
 *  and a mark most of them wear is a mark none of them makes. Medium is the
 *  ordinary case and says nothing here. */
export const priorityIsPressing = (priority) => priority === "high" || priority === "urgent";

/** Open or closed, which is independent of the Done column: one says whether
 *  the work is still live, the other where it stands on the board, and the SPA
 *  shows both. */
export const stateLabel = (state) => (state === "closed" ? "Closed" : "Open");

// ---- actors ---------------------------------------------------------------
//
// One tagged shape everywhere a person or an agent is named — an assignee, a
// comment's author, an event's actor. `project_agent` is only ever an
// assignee; an author and an actor are always `user` or `agent`.

/** The key an unassigned task stands under. The wire's own word for it —
 *  `tasks.list` takes `"none"` beside the actor shapes — so the filter bar's
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

/* What to call an actor lives in core/trackerLineWords.js, with the rest of the
   tracker's vocabulary: `actorName(actor, { agentLabels, projectName })`. It
   was here as well for a while, and the two drifted — the copy here had never
   heard of the project's own agent, so the same actor read "Build agent" on a
   notice line and "Agent 01M2" on its own comment (#63). */

/** The initials an avatar wears for an actor: the human's, the project agent's,
 *  and an agent's own. Never more than two characters — it sits in a circle. */
export function actorInitials(actor) {
  if (!actor || !actor.kind) return "–";
  if (actor.kind === "user") return "Y";
  if (actor.kind === "project_agent") return "P";
  if (actor.kind === "build") return "B";
  return "A";
}

/** A task's links, with every list present. A record written by a bridge that
 *  left one out reads as empty rather than as undefined, so every rail row can
 *  be drawn without asking whether it has anything to draw. */
export function taskLinks(task) {
  const links = task?.links || {};
  return {
    workspace_ids: links.workspace_ids || [],
    branches: links.branches || [],
    commits: links.commits || [],
    conversation_ids: links.conversation_ids || [],
    parent_task_id: links.parent_task_id || null,
  };
}

/** Whether a task's row on the list is newer than its cached `tasks.get`
 *  record — which is when that record has to be read again, and when its
 *  timeline no longer speaks for the task. */
export const changedSince = (listTask, detail) => {
  if (!detail?.task) return true;
  const listedAt = listTask?.updated_at;
  if (!listedAt) return false;
  const detailedAt = detail.task.updated_at;
  if (!detailedAt) return true;
  const listTime = Date.parse(listedAt);
  const detailTime = Date.parse(detailedAt);
  return Number.isFinite(listTime) && Number.isFinite(detailTime)
    ? listTime > detailTime
    : listedAt !== detailedAt;
};
