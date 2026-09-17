// Adopt-on-first-mutation: a run-RPC caller bound to one checkout that
// transparently mints a run (run.adopt) the first time a mutating action runs,
// then routes every call through the adopted run_id. Adoption happens at most
// once; a refused adopt leaves the checkout un-adopted so the next action retries.
//
// One checkout adopts this way: an external worktree (`worktree_id`), which is
// the only kind of checkout there is to claim. A project's own checkout is the
// template its workspaces are cut from, and the bridge refuses an adopt of it.

import { replyOrNothing } from "./session.js";

/** How long to wait before asking the daemon again whether the adoption it is
 *  running has landed. The daemon runs an adopt's git — a scan, a checkpoint
 *  commit, a scaffold — behind its answer, so the answer can outlive the RPC
 *  timer while the adoption succeeds; and an adopt that arrives while that git
 *  runs is told so and handed no run, since none is durable yet. Both read the
 *  same way: the checkout is being claimed, ask again. */
export const ADOPT_REASK_MS = 2000;
/** How many times to ask again before giving up on the daemon ever naming the
 *  run. A bound, so a daemon that never answers is an error and not a poll. */
export const ADOPT_REASK_LIMIT = 30;

const wait = (ms) => new Promise((done) => setTimeout(done, ms));
/** The `run.adopt` params naming one external worktree — the shape both the
 *  transparent adopter and an explicit adoption send, defined once. */
export const worktreeAdoptScope = (projectId, worktreeId) => ({ project_id: projectId, worktree_id: worktreeId });

/** A run-RPC caller for one checkout that transparently adopts on first use.
 *  `adoptScope` is the run.adopt params naming that checkout. */
function createScopedAdoptingCall(call, adoptScope) {
  let runId = null;
  let adoptInFlight = null;
  let adoptParams = {};

  /** Ask until the daemon names the run that owns this checkout. Only a
   *  refusal ends the asking early: a reply the timer cut short, or one that
   *  names no run, is the adoption still running. */
  const adoptUntilNamed = async () => {
    for (let asked = 0; ; asked += 1) {
      const view = await replyOrNothing(call("run.adopt", { ...adoptScope, ...adoptParams }));
      if (view && view.run_id) return view.run_id;
      if (asked >= ADOPT_REASK_LIMIT) throw new Error("run.adopt never answered with a run");
      await wait(ADOPT_REASK_MS);
    }
  };

  const ensureAdopted = () => {
    if (runId) return Promise.resolve(runId);
    if (!adoptInFlight) {
      adoptInFlight = adoptUntilNamed().then(
        (named) => {
          runId = named;
          return runId;
        },
        (error) => {
          adoptInFlight = null; // a refused adopt must not stick — let a retry re-adopt
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

/** A run-RPC caller for one external worktree. `call` is a device context's
 *  call (injected for tests). */
export function createAdoptingCall(call, projectId, worktreeId) {
  return createScopedAdoptingCall(call, worktreeAdoptScope(projectId, worktreeId));
}

/** What checkout a scope names, as one string — the key an adopter is kept
 *  under. A scope Build already owns (a run) needs no adopter and has no key,
 *  and neither has one that names no worktree: an external worktree is the only
 *  thing left to adopt. */
function checkoutKey(scope) {
  if (!scope || !scope.project_id || scope.run_id || !scope.worktree_id) return null;
  return `worktree:${scope.worktree_id}`;
}

/**
 * The adopters of one view: one per checkout, made on demand and kept.
 *
 * A view can have more than one surface able to mutate first — the branch view
 * has two, the agent rail and the Changes review — and an adopter each would
 * race to mint two owners of the same checkout. `createAdopters` hands them
 * all the same one.
 *
 * `call` is a device context's call (injected for tests). The returned lookup
 * takes a git scope (views/branchView.js `branchScope`) and answers the adopter
 * for it, or null where nothing is to be adopted: a run already owns that
 * checkout, or the row names none.
 */
export function createAdopters(call) {
  const byCheckout = new Map();
  return (scope) => {
    const key = checkoutKey(scope);
    if (!key) return null;
    if (!byCheckout.has(key)) {
      byCheckout.set(key, createAdoptingCall(call, scope.project_id, scope.worktree_id));
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
