// Adopt-on-first-mutation: a task-RPC caller bound to one external worktree that
// transparently mints a task (task.adopt) the first time a mutating action runs,
// then routes every call through the adopted task_id. Adoption happens at most
// once; a failed adopt leaves the worktree un-adopted so the next action retries.

/** A task-RPC caller for one external worktree that transparently adopts on
 *  first use. `call` is App.call-shaped (injected for tests). Adoption runs at
 *  most once; a failed adopt stays un-adopted so the next action retries. */
export function createAdoptingCall(call, projectId, worktreeId) {
  let taskId = null;
  let adoptInFlight = null;

  const ensureAdopted = () => {
    if (taskId) return Promise.resolve(taskId);
    if (!adoptInFlight) {
      adoptInFlight = call("task.adopt", { project_id: projectId, worktree_id: worktreeId }).then(
        (view) => {
          taskId = view.task_id;
          return taskId;
        },
        (error) => {
          adoptInFlight = null; // a failed adopt must not stick — let a retry re-adopt
          throw error;
        },
      );
    }
    return adoptInFlight;
  };

  return {
    async taskCall(method, params) {
      const id = await ensureAdopted();
      return call(method, { task_id: id, ...(params || {}) });
    },
    adoptedTaskId() {
      return taskId;
    },
  };
}
