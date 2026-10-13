// Readiness is an authoritative cached task fact. Missing blocker records or
// a running holder cannot override it, and rendering never needs a greeting.
import { PRIORITIES } from "./trackerModel.js";

const UNDELIVERED = new Set(["waiting", "queued", "provisioning", "failed"]);
export const taskIsBlocked = (task) => task?.dependencies?.blocked === true;
export const taskHasReadiness = (task) => Boolean(task?.dependencies || task?.deferred_assignment);
export const taskHasUndeliveredAssignment = (task) => UNDELIVERED.has(task?.deferred_assignment?.state);
export const taskIsHeld = (task) => Boolean(task?.assignee?.kind ||
  (taskHasUndeliveredAssignment(task) && task.deferred_assignment.target?.kind));

/** Called after history and holder placement. A blocked assignment is never
 * Working, including after its consumed delivery is reblocked. */
export function taskActivityGroup(task, working) {
  if (taskIsBlocked(task)) return "blocked";
  return working === true && !taskHasUndeliveredAssignment(task) ? "working" : "assigned";
}

export const unresolvedTaskBlockers = (task) =>
  (task?.dependencies?.waiting_on || []).filter((blocker) => blocker.resolved === false);

export const taskWaitingText = (task) => {
  if (!taskIsBlocked(task)) return "";
  const blockers = unresolvedTaskBlockers(task);
  return blockers.length ? `Waiting on ${blockers.map((blocker) => `#${blocker.number}`).join(", ")}` : "Waiting on prerequisites";
};

const priorityRank = (task) => Math.max(0, PRIORITIES.findIndex((priority) => priority.id === task?.priority));
export const tasksByPriority = (tasks) => [...(tasks || [])].sort((a, b) => priorityRank(b) - priorityRank(a));

/** Backlog headings are a projection, never a stored column or label write. */
export function taskReadinessGroups(tasks) {
  const ordered = tasksByPriority(tasks);
  return [
    { id: "blocked", title: "Blocked", tasks: ordered.filter(taskIsBlocked) },
    { id: "ready", title: "Ready", tasks: ordered.filter((task) => !taskIsBlocked(task)) },
  ].filter((group) => group.tasks.length);
}
