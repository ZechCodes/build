// Adopt-on-first-mutation: a run-RPC caller bound to one checkout that
// transparently mints a run (run.adopt) the first time a mutating action runs,
// then routes every call through the adopted run_id. Adoption happens at most
// once; a failed adopt leaves the checkout un-adopted so the next action retries.
//
// Two checkouts adopt this way and they take the same path: an external
// worktree (`worktree_id`) and the project's primary checkout (`primary: true`),
// the repo root as a super-worktree. Only the scope the adopt names differs.

/** A run-RPC caller for one checkout that transparently adopts on first use.
 *  `adoptScope` is the run.adopt params naming that checkout. */
function createScopedAdoptingCall(call, adoptScope) {
  let runId = null;
  let adoptInFlight = null;
  let adoptParams = {};

  const ensureAdopted = () => {
    if (runId) return Promise.resolve(runId);
    if (!adoptInFlight) {
      adoptInFlight = call("run.adopt", { ...adoptScope, ...adoptParams }).then(
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
    setAdoptParams(params) {
      if (!runId && !adoptInFlight) adoptParams = params || {};
    },
    /** Adopt-by-reconnect: bind to a run that already owns this checkout
     *  (learned read-only, e.g. from the feed) so opening the surface after a
     *  reload routes through it instead of minting a second owner. */
    seedAdoptedRun(id) {
      if (id && !runId && !adoptInFlight) runId = id;
    },
    /** The bound run reached a terminal state: it has let go of the checkout,
     *  which is adoptable again, so the next action must mint a new owner. */
    releaseAdoptedRun() {
      runId = null;
      adoptInFlight = null;
    },
    async runCall(method, params) {
      const id = await ensureAdopted();
      return call(method, { run_id: id, ...(params || {}) });
    },
    adopt: ensureAdopted,
    adoptedRunId() {
      return runId;
    },
  };
}

/** A run-RPC caller for one external worktree. `call` is App.call-shaped
 *  (injected for tests). */
export function createAdoptingCall(call, projectId, worktreeId) {
  return createScopedAdoptingCall(call, { project_id: projectId, worktree_id: worktreeId });
}

/** A run-RPC caller for a project's primary checkout — the repo root, adopted
 *  as a super-worktree. The bridge enforces one owner per project, so a reload
 *  or a second browser converges on the run that already owns it. */
export function createPrimaryAdoptingCall(call, projectId) {
  return createScopedAdoptingCall(call, { project_id: projectId, primary: true });
}

/** What checkout a scope names, as one string — the key an adopter is kept
 *  under. A scope Build already owns (a run) needs no adopter and has no key. */
function checkoutKey(scope) {
  if (!scope || !scope.project_id || scope.run_id) return null;
  return scope.worktree_id ? `worktree:${scope.worktree_id}` : `primary:${scope.project_id}`;
}

/**
 * The adopters of one view: one per checkout, made on demand and kept.
 *
 * A view can have more than one surface able to mutate first — the branch view
 * has two, the agent rail and the Changes review — and an adopter each would
 * race to mint two owners of the same checkout. `createAdopters` hands them
 * all the same one.
 *
 * `call` is App.call-shaped (injected for tests). The returned lookup takes a
 * git scope (views/branchView.js `branchScope`) and answers the adopter for it,
 * or null where nothing is to be adopted: a run already owns that checkout, or
 * the row names none.
 */
export function createAdopters(call) {
  const byCheckout = new Map();
  return (scope) => {
    const key = checkoutKey(scope);
    if (!key) return null;
    if (!byCheckout.has(key)) {
      byCheckout.set(
        key,
        scope.worktree_id
          ? createAdoptingCall(call, scope.project_id, scope.worktree_id)
          : createPrimaryAdoptingCall(call, scope.project_id),
      );
    }
    return byCheckout.get(key);
  };
}

/** Start a checkout's agent from the Agent tab, adopting the checkout on the
 *  way (an agent needs an owner for `done` to report to).
 *
 *  `pickedProvider` is the card the human pressed, `markedProvider` the answer
 *  the new-worktree sheet already got — a press is the later and more explicit
 *  of the two, so it wins. The provider seeds the adopt (the run is minted on
 *  it) AND rides the start, so a checkout adopted earlier still switches. */
export function startAdoptedAgent(adopting, pickedProvider, markedProvider) {
  const provider = pickedProvider || markedProvider;
  if (provider) adopting.setAdoptParams({ provider });
  return adopting.runCall("agent.start", provider ? { provider } : {});
}
