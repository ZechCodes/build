// A task's unread, counted the way an agent's is (#104).
//
// Zech: "Tasks should be treated as agents when computing unread counts." And:
// "When not watched no unread count should ever be shown." So only a watched
// task has one. What it counts is #99's unread line — every timeline entry
// after the user's read mark that is not their own (core/trackerUnread.js) —
// read off the cached records: the task's cached timeline while it is at least
// as new as the list's row, which is where a read this tab just made shows
// first, and otherwise the `unread_count` the bridge puts on each watched row
// of `tasks.list` (wire 1.29.0), by the same rule.
//
// Where the count is worn:
//   • the task's own bubble, on the board, the list and the dashboard;
//   • the Tasks tab: the project's carries every watched task's, a
//     workspace's only those its agents hold;
//   • the rail: a task an agent holds counts on that agent's workspace row,
//     and every other one — nobody's, the user's, the project agent's, or an
//     agent's whose workspace has no row to wear it — on the project's own.
//
// No DOM, no app imports.

import { isFinished } from "./trackerAgentTasks.js";
import { changedSince } from "./trackerModel.js";
import { timelineRows } from "./trackerTimeline.js";
import { taskUnreadReading, latestTaskMark } from "./trackerUnread.js";

const listedCount = (task) => (Number.isSafeInteger(task?.unread_count) ? task.unread_count : null);

/** #99's count over a cached timeline, from the newer of the two read marks. */
function timelineUnread(task, detail) {
  if (!Array.isArray(detail?.timeline)) return 0;
  const mark = latestTaskMark(detail.task?.read_through, task?.read_through);
  return taskUnreadReading(timelineRows(detail.timeline), mark).unreadCount;
}

/** How much of one task is unread: `task` as the cached list holds it,
 *  `detail` its cached `tasks.get` record when there is one. */
export function taskUnreadCount(task, detail = null) {
  if (task?.watched !== true) return 0;
  const listed = listedCount(task);
  const timelineIsCurrent = Array.isArray(detail?.timeline) && !changedSince(task, detail);
  return timelineIsCurrent || listed === null ? timelineUnread(task, detail) : listed;
}

/** The bubble a task wears where tasks are listed, and nothing at all for
 *  none. The inbox's own badge, so an unread count reads the same anywhere. */
export const unreadBubbleHtml = (count) =>
  (count > 0 ? `<span class="badge inbox-unread task-unread" title="${count} unread">${count}</span>` : "");

/** One task's bubble: `unreadOf` is the count the listing surface keeps
 *  (the Tasks pane reads its cached timelines), the list's own otherwise. */
export const taskBubbleHtml = (task, unreadOf = taskUnreadCount) => unreadBubbleHtml(unreadOf(task));

/** Whether a task's unread goes into a total (#183): a Done or closed one
 *  never does, whatever the list says — a 1.29.0 bridge puts `unread_count`
 *  on every watched row, finished ones too. Its own bubble still shows it.
 *  The one rule every total reads, so the Tasks tabs and the rail agree. */
export const countsInTotals = (task) => !isFinished(task);

/** The unread over a list of tasks, as a Tasks tab wears it: `detailOf`
 *  finds a task's cached timeline, `only` keeps the ones the tab is about. */
export function watchedTasksUnread(tasks = [], { detailOf = () => null, only = () => true } = {}) {
  return tasks.reduce((total, task) =>
    (countsInTotals(task) && only(task) ? total + taskUnreadCount(task, detailOf(task)) : total), 0);
}

/** The agent holding a task, when an agent does. */
const holderOf = (task) => (task?.assignee?.kind === "agent" ? task.assignee.agent_id || null : null);

const sumOf = (held) => held.reduce((total, one) => total + one.count, 0);

/**
 * The rail's reading of every followed project's watched tasks:
 * `sources` is `[{ project, tasks, details }]`, as core/watchedTaskFollower.js
 * holds them.
 *
 * A Done or closed task never counts here (#183, countsInTotals).
 *
 * `heldBy(projectKey, agentIds)` is what one workspace row wears: the tasks
 * its agents hold. `unheldBy(rows)` answers, per project key, what the
 * project's own badge wears: every task none of those workspace rows' agents
 * hold, `rows` being `[{ projectKey, agentIds }]`.
 */
export function taskUnreadTally(sources = []) {
  const byProject = new Map();
  for (const { project, tasks = [], details = new Map() } of sources) {
    const held = tasks
      .filter(countsInTotals)
      .map((task) => ({ holder: holderOf(task), count: taskUnreadCount(task, details.get(task.id) || null) }))
      .filter((one) => one.count > 0);
    byProject.set(project.projectKey, held);
  }
  const heldIn = (projectKey) => byProject.get(projectKey) || [];
  return {
    heldBy(projectKey, agentIds = []) {
      const ids = new Set(agentIds);
      return sumOf(heldIn(projectKey).filter((one) => ids.has(one.holder)));
    },
    unheldBy(rows = []) {
      return (projectKey) => {
        const ids = new Set(rows.filter((row) => row.projectKey === projectKey).flatMap((row) => row.agentIds || []));
        return sumOf(heldIn(projectKey).filter((one) => !ids.has(one.holder)));
      };
    },
  };
}

/** The reading of a rail that follows no tasks. */
export const NO_TASK_UNREAD = taskUnreadTally();
