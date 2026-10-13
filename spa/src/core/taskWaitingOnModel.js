// Pure task-page dependency decisions. The graph and delivery intent are saved
// bridge facts; an assignee alone never implies a pending worker.
export const waitingOnEdges = (task) => task?.dependencies?.waiting_on || [];
const unresolvedEdges = (task) => waitingOnEdges(task).filter((edge) => !edge.resolved);
const candidateId = (task) => task.id || task.task_id;

export function blockerChoices(task, tasks, projectId, query = "") {
  const linked = new Set([task?.id, ...waitingOnEdges(task).map((edge) => edge.task_id)]);
  const search = query.trim().toLocaleLowerCase().replace(/^#/, "");
  return (tasks || []).filter((candidate) =>
    (!candidate.project_id || candidate.project_id === projectId)
      && !linked.has(candidateId(candidate))
      && `${candidate.number} ${candidate.title}`.toLocaleLowerCase().includes(search));
}

const lastUnresolvedEdge = (task, blockerId) => {
  const edges = unresolvedEdges(task);
  return edges.length === 1 && edges[0].task_id === blockerId;
};

function unsavedRemovalConsequence(task, saved) {
  if (task?.assignee?.kind === "user") return "Removing the last blocker does not start an agent for a user-held task.";
  if (saved?.state === "failed") return "Removing the last blocker leaves the failed saved assignment available to retry. Retry it explicitly to request delivery.";
  if (task?.assignee || ["cancelled", "consumed"].includes(saved?.state)) return "Removing the last blocker does not start another turn. Assign again to request another turn.";
  return "Removing the last blocker creates no worker: this task has no saved assignment.";
}

export function removalConsequence(task, blockerId) {
  if (!lastUnresolvedEdge(task, blockerId)) return "Remove this Waiting on link?";
  const saved = task?.deferred_assignment;
  if (saved && ["waiting", "queued"].includes(saved.state) && task?.assignee?.kind !== "user") {
    return "Removing the last blocker starts the saved assignment when the bridge can deliver it.";
  }
  return unsavedRemovalConsequence(task, saved);
}

const SAVED_NOTICES = {
  waiting: "Assignment saved. It starts after every prerequisite is Done or closed. The receiving agent assesses closed prerequisites before proceeding.",
  queued: "Assignment saved and queued for delivery.",
  failed: "The saved assignment could not be delivered. Retry uses the same saved assignment.",
};
const startedTaskNotice = (task) => task?.dependencies?.blocked && ["in_progress", "in_review"].includes(task.status)
  ? "A prerequisite is unresolved after work started. Reopening a prerequisite does not stop a running turn or start another turn. Assign again to request another turn." : "";

export function waitingOnNotice(task) {
  return SAVED_NOTICES[task?.deferred_assignment?.state] || startedTaskNotice(task);
}

export const edgeResolutionLabel = (edge) => {
  if (edge.resolved_by === "done") return "Done · resolved";
  if (edge.resolved_by === "close") return "Closed · resolved";
  return "Waiting";
};
