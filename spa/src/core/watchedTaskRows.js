// Every watched open task stays in Inbox until Done, closed, or unwatched.
// Attention reasons explain the rows that need the user; every row keeps the
// same anchor order and Recent rule as workspaces, regardless of attention.
//
// Read from the cached task records, never the board feed's `tracker_task`
// rows: an `tasks` push re-reads the project's list and nothing re-reads the
// board for it, so the list is the record that moves when the task does.
//
// No DOM, no app imports — the wiring (core/watchedTaskFollower.js) reads the
// cache and core/inboxView.js paints these beside the workspace rows.

import { TRACKER_TASK, byAnchor, entryKeyOf } from "./inbox.js";
import { isFinished } from "./trackerAgentTasks.js";
import { taskUnreadCount } from "./taskUnread.js";
import { ATTENTION_REASONS, watchedTaskReasons } from "./trackerAttentionModel.js";

/** Why the row is there, in the inbox's words. */
const REASON_WORDS = Object.freeze({
  [ATTENTION_REASONS.inReview]: "In review",
  [ATTENTION_REASONS.assigned]: "Assigned to you",
});

const reasonWord = (reason, askedOnly) => reason === ATTENTION_REASONS.inbox
  ? (askedOnly ? "Mentioned you" : "New comment") : REASON_WORDS[reason];

const ms = (iso) => {
  const parsed = Date.parse(iso || "");
  return Number.isFinite(parsed) ? parsed : null;
};

const titleOf = (task) => task.title || "(untitled)";

/** Creation anchors a task without a bridge anchor. Cached comments and
 *  events can renew its activity without moving that anchor. */
function taskTimes(task, detail) {
  const activity = [task.created_at, task.updated_at, task.last_activity,
    ...(detail?.timeline || []).map((item) => item.created_at || item.at)]
    .map(ms).filter(Number.isFinite);
  const lastActivityMs = activity.reduce((latest, value) => Math.max(latest ?? value, value), null);
  return { anchorMs: ms(task.anchor) ?? ms(task.created_at) ?? lastActivityMs, lastActivityMs };
}

/** The fields a row carries about a checkout, which a task has none of. */
const NOT_A_CHECKOUT = Object.freeze({
  branch: null,
  pending: null,
  placeholder: false,
  canFinish: false,
  merged: false,
  muted: false,
  dismissed: false,
  warnings: [],
});

/** A task follows the running state of the agent it is assigned to. */
export function taskAgentIsRunning(task, runningAgentIds, projectAgentId = null) {
  const assignee = task?.assignee;
  const agentId = assignee?.kind === "project_agent" ? projectAgentId
    : assignee?.kind === "agent" ? assignee.agent_id : null;
  return runningAgentIds.has(agentId);
}

function toEntry(project, task, detail, reasons, askedOnly, runningAgentIds, projectAgentId) {
  const facts = reasons.map((reason) => reasonWord(reason, askedOnly)).join(" · ");
  const unreadCount = taskUnreadCount(task, detail);
  const working = taskAgentIsRunning(task, runningAgentIds, projectAgentId);
  return {
    ...NOT_A_CHECKOUT,
    key: entryKeyOf({ kind: TRACKER_TASK, task_id: task.id }),
    kind: TRACKER_TASK,
    // A task is not an entity the board's read and mute verbs know; its own
    // verbs are `tasks.*`, named by the task id.
    entityId: null,
    taskId: task.id,
    number: task.number ?? null,
    deviceId: project.deviceId,
    projectId: project.id,
    projectKey: project.projectKey,
    project: project.name || project.id,
    name: task.number ? `#${task.number} ${titleOf(task)}` : titleOf(task),
    title: titleOf(task),
    reviewSummary: task.review_summary || null,
    // Attention, running, and unread are independent facts about a watched task.
    state: working ? "working" : unreadCount > 0 ? "unread" : "inactive",
    working,
    quiet: reasons.length === 0,
    reason: facts,
    facts,
    unreadCount,
    // Watch confirmation follows record updates, independently of row order.
    updatedMs: ms(task.updated_at),
    route: { name: "trackerTask", deviceId: project.deviceId, projectId: project.id, taskId: task.id },
    ...taskTimes(task, detail),
  };
}

function entriesFromSource({ project, tasks = [], details = new Map(), askedOnly = false, runningAgentIds = new Set(), projectAgentId = null }) {
  const entries = [];
  for (const task of tasks) {
    if (task.watched !== true || isFinished(task)) continue;
    const detail = details.get(task.id) || null;
    const reasons = watchedTaskReasons(task, detail, askedOnly);
    entries.push(toEntry(project, task, detail, reasons, askedOnly, runningAgentIds, projectAgentId));
  }
  return entries;
}

/**
 * The rows, from each followed project's cached records: `sources` is
 * `[{ project, tasks, details, askedOnly, runningAgentIds, projectAgentId }]`.
 * `project` is the feed's project (`id`, `deviceId`, `projectKey`, `name`), `tasks` the cached
 * `tasks.list`, `details` the cached `tasks.get` answers by task id, and
 * `askedOnly` the machine's cached Needs you rule (core/needsYouRule.js), and
 * `runningAgentIds` the cached agent lineage's running members, and
 * `projectAgentId` the cached project owner's holder for legacy assignments.
 */
export function watchedTaskEntries(sources = []) {
  return sources.flatMap(entriesFromSource).sort(byAnchor);
}
