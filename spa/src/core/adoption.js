// Adopt-on-first-mutation: a run-RPC caller bound to one external worktree that
// transparently mints a run (run.adopt) the first time a mutating action runs,
// then routes every call through the adopted run_id. Adoption happens at most
// once; a failed adopt leaves the worktree un-adopted so the next action retries.

/** A run-RPC caller for one external worktree that transparently adopts on
 *  first use. `call` is App.call-shaped (injected for tests). Adoption runs at
 *  most once; a failed adopt stays un-adopted so the next action retries. */
export function createAdoptingCall(call, projectId, worktreeId) {
  let runId = null;
  let adoptInFlight = null;

  const ensureAdopted = () => {
    if (runId) return Promise.resolve(runId);
    if (!adoptInFlight) {
      adoptInFlight = call("run.adopt", { project_id: projectId, worktree_id: worktreeId }).then(
        (view) => {
          runId = view.run_id;
          return runId;
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
    async runCall(method, params) {
      const id = await ensureAdopted();
      return call(method, { run_id: id, ...(params || {}) });
    },
    adoptedRunId() {
      return runId;
    },
  };
}
