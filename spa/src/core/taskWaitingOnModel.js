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

export function removalConsequence(task, blockerId) {
  const remaining = unresolvedEdges(task);
  const last = remaining.length === 1 && remaining[0].task_id === blockerId;
  const saved = task?.deferred_assignment;
  if (last && saved && ["waiting", "queued"].includes(saved.state)) {
    return "Removing the last blocker starts the saved assignment when the bridge can deliver it.";
  }
  return "Remove this Waiting on link?";
}

export function waitingOnNotice(task) {
  const saved = task?.deferred_assignment;
  if (saved?.state === "waiting") return "Assignment saved. It starts after every prerequisite is Done or closed. The receiving agent assesses closed prerequisites before proceeding.";
  if (saved?.state === "queued") return "Assignment saved and queued for delivery.";
  if (saved?.state === "failed") return "The saved assignment could not be delivered. Retry uses the same saved assignment.";
  if (task?.dependencies?.blocked && ["in_progress", "in_review"].includes(task.status)) {
    return "A prerequisite is unresolved after work started. Reopening a prerequisite does not stop a running turn or start another turn. Assign again to request another turn.";
  }
  return "";
}

export const edgeResolutionLabel = (edge) => {
  if (edge.resolved_by === "done") return "Done · resolved";
  if (edge.resolved_by === "close") return "Closed · resolved";
  return "Waiting";
};
