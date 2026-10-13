// What one agent's tasks are, grouped the way a reader of its conversation
// wants them.
//
// What it is working on, what it is holding, what is blocked, what
// it has finished, and what it is only watching. The order is the reader's
// question order — "what is this agent doing" first, "what did it do" last —
// and the two at the bottom are collapsed because they are history and
// background rather than news.
//
// An empty group is not drawn at all. A group with a heading and nothing under
// it tells the reader there is a category they have none of, which is not
// something anybody needs to be told repeatedly.
//
// This is also the grouping the workspace's tasks view uses per agent (#16),
// which is why it takes an agent id rather than reading one: the same four
// questions asked of each agent in a workspace give that view its sections.
//
// No DOM, no app imports.

import { taskActivityGroup, taskHasReadiness, tasksByPriority, unresolvedTaskBlockers } from "./trackerTaskReadiness.js";

/** The column a task is being worked in. The one status that means "this is
 *  happening now" rather than "this is somewhere on the board". */
const IN_PROGRESS = "in_progress";

/** The column that means the work is over. Independent of open/closed — a
 *  task can be closed anywhere on the board, and can sit in Done still open —
 *  so both are asked, and either one makes it history. */
const DONE = "done";

export const WORKING_GROUP = "working";
export const HOLDING_GROUP = "holding";
export const BLOCKED_GROUP = "blocked";
export const FINISHED_GROUP = "finished";
export const WATCHING_GROUP = "watching";

/** Whether this task names this agent as its assignee. The tagged actor shape
 *  is the wire's (core/trackerModel.js); only the `agent` kind can be one of
 *  these, since a project agent and the user are not agents of a workspace. */
export const assignedTo = (task, agentId) =>
  Boolean(agentId) && task?.assignee?.kind === "agent" && task.assignee.agent_id === agentId;

/**
 * Whether this agent is following the task.
 *
 * `trackers` is the list #13 puts on the record. A bridge that predates it
 * sends no such field, and every task then reads as untracked — which is the
 * right answer for a bridge with no tracking in it, and means the Tracking
 * group simply never appears rather than the entry breaking.
 */
export const tracks = (task, agentId) =>
  Boolean(agentId) && (task?.trackers || []).includes(agentId);

/** Whether the work is over: closed anywhere, or sitting in Done. Both, because
 *  the two are independent and either one makes the task history to a reader
 *  looking at what an agent is doing now. */
export const isFinished = (task) => task?.state === "closed" || task?.status === DONE;

/**
 * Which group a task falls in for this agent, or null for one that is
 * neither assigned to it nor tracked by it.
 *
 * Assignment wins over tracking: the assignee is tracked automatically (#13),
 * so almost every assigned task is also a tracked one, and showing it twice
 * would double every row. Watching is therefore what this agent follows and
 * does NOT hold.
 */
export function groupOf(task, agentId, { working = false } = {}) {
  if (assignedTo(task, agentId)) return heldGroupOf(task, working);
  return tracks(task, agentId) ? WATCHING_GROUP : null;
}

function heldGroupOf(task, working) {
  if (isFinished(task)) return FINISHED_GROUP;
  if (!taskHasReadiness(task)) return task.status === IN_PROGRESS ? WORKING_GROUP : HOLDING_GROUP;
  const activity = taskActivityGroup(task, working);
  if (activity === "blocked") return BLOCKED_GROUP;
  return activity === "working" ? WORKING_GROUP : HOLDING_GROUP;
}

/** When a task last moved, as a number. A task with no readable stamp
 *  sorts oldest rather than throwing the order out: 0 is "unknown", not "now". */
const movedAt = (task) => {
  const at = Date.parse(task?.updated_at || "");
  return Number.isFinite(at) ? at : 0;
};

/**
 * Most recently moved first, and by number within a tie.
 *
 * NOT the Tasks tab's order. That list answers newest-filed first because it
 * is a catalogue of the project; this is an activity entry, where the question
 * is what has moved, and each card wears the time it moved for exactly that
 * reason. The number breaks ties so two tasks touched in the same write still
 * have one order every repaint agrees on.
 */
export const byRecency = (left, right) =>
  movedAt(right) - movedAt(left) || Number(right?.number || 0) - Number(left?.number || 0);

// ---- the agent's tasks as one of its surfaces ----------------------------
//
// #34. These four questions used to answer a block drawn inline in the
// conversation, and the maintainer read it as noise: "I had envisioned the
// tasks activity to be the same UX as agents/workflows/tasks/shells. So it's
// only visible when the user wants it to be and there's a clear pattern for
// done work."
//
// So they answer a different shape now: one flat list of rows the surfaces
// layer can draw (core/agentSurfacesModel.js), each carrying the state that
// decides whether it sits above the fold or under it.
//
// What "finished" means is wider here than it is for the badge. On an AGENT's
// list, In review is finished: the agent has said the work is ready to be
// looked at and has nothing more to do with it, and leaving it above the fold
// is what made this read as noise. It is still open, and the Tasks tab still
// lists it as open — that is a different question, asked by a different
// reader.

export const TASK_SURFACE_IN_PROGRESS = "in_progress";
export const TASK_SURFACE_WORKING = "working";
export const TASK_SURFACE_ASSIGNED = "assigned";
export const TASK_SURFACE_BLOCKED = "blocked";
export const TASK_SURFACE_TRACKED = "tracked";
export const TASK_SURFACE_IN_REVIEW = "in_review";
export const TASK_SURFACE_DONE = "done";
export const TASK_SURFACE_CLOSED = "closed";

const IN_REVIEW = "in_review";

/** The order the rows read in: what it is doing, what it is holding, what it
 *  is only following, then the three it is finished with. */
const SURFACE_STATE_ORDER = [
  TASK_SURFACE_WORKING,
  TASK_SURFACE_IN_PROGRESS,
  TASK_SURFACE_ASSIGNED,
  TASK_SURFACE_BLOCKED,
  TASK_SURFACE_TRACKED,
  TASK_SURFACE_IN_REVIEW,
  TASK_SURFACE_DONE,
  TASK_SURFACE_CLOSED,
];

/**
 * Where one task stands for this agent, as the surfaces layer names states.
 *
 * Closed is asked before the column because it is true wherever the card sits:
 * a task closed from Backlog is finished, and calling it "assigned" would
 * put work nobody is doing at the top of the list.
 */
function surfaceStateOf(task, agentId, options) {
  const group = groupOf(task, agentId, options);
  if (group === null) return null;
  if (group === WATCHING_GROUP) return TASK_SURFACE_TRACKED;
  const history = historyStateOf(task);
  if (history) return history;
  if (group === BLOCKED_GROUP) return TASK_SURFACE_BLOCKED;
  return activityStateOf(task, group);
}

function activityStateOf(task, group) {
  if (group !== WORKING_GROUP) return TASK_SURFACE_ASSIGNED;
  return taskHasReadiness(task) ? TASK_SURFACE_WORKING : TASK_SURFACE_IN_PROGRESS;
}

function historyStateOf(task) {
  if (task.state === "closed") return TASK_SURFACE_CLOSED;
  if (task.status === DONE) return TASK_SURFACE_DONE;
  return task.status === IN_REVIEW ? TASK_SURFACE_IN_REVIEW : null;
}

/**
 * One agent's tasks as surface entries: flat, ordered, ready to draw.
 *
 * Readiness-aware lists keep priority first and source order within ties.
 * Older records retain the established most-recently-moved order.
 *
 * Deliberately NOT the raw task record: a task's own `state` is open or
 * closed, and the surfaces layer reads `state` as the mark that decides where
 * a row sits. Two meanings on one field is a bug waiting for somebody to read
 * the wrong one.
 */
export function agentTaskEntries(tasks, agentId, options = {}) {
  const enabled = (tasks || []).some(taskHasReadiness);
  const orderedTasks = enabled ? tasksByPriority(tasks) : tasks;
  return (orderedTasks || []).map((task, index) => ({ task, index, state: surfaceStateOf(task, agentId, options) }))
    .filter(({ state }) => state)
    .sort((left, right) => surfaceEntryOrder(left, right, enabled))
    .map(({ task, state }) => ({
      id: task.id,
      state,
      number: task.number ?? null,
      title: task.title || "",
      status: task.status || "",
      updated_at: task.updated_at || null,
      ...(taskHasReadiness(task) ? { readinessAware: true, projectId: options.projectId || task.project_id, deviceId: options.deviceId || null } : {}),
      ...(state === TASK_SURFACE_BLOCKED ? { blockers: unresolvedTaskBlockers(task) } : {}),
    }));
}

function surfaceEntryOrder(left, right, enabled) {
  const standing = SURFACE_STATE_ORDER.indexOf(left.state) - SURFACE_STATE_ORDER.indexOf(right.state);
  if (standing) return standing;
  if (!enabled) return byRecency(left.task, right.task);
  return left.index - right.index;
}

/** The open tasks this agent holds — not finished, not merely tracked. What a
 *  badge counts when it is asking "is this agent carrying anything". */
export const agentOpenTaskCount = (tasks, agentId) =>
  (tasks || []).filter((task) => {
    const group = groupOf(task, agentId);
    return [WORKING_GROUP, HOLDING_GROUP, BLOCKED_GROUP].includes(group);
  }).length;
