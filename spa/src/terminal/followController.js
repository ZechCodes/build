const sameTarget = (left, right) => Boolean(
  left && right
  && left.deviceId === right.deviceId
  && left.context === right.context
  && left.carrier === right.carrier
);

const publicState = (phase, identity = null) => ({
  phase,
  deviceId: identity?.deviceId ?? null,
  sessionId: identity?.sessionId ?? null,
  carrier: identity?.carrier ?? null,
});

/**
 * Own the terminal follow transition as one device/context/session/carrier
 * identity. The manager supplies policy and effects; this object alone decides
 * which asynchronous result still has authority to change the terminal RPC.
 */
export function createTerminalFollowController({ mint, adopt, ride, detach, isDesired }) {
  let adopted = null;
  let pending = null;
  let authority = null;

  const current = (transition) => pending === transition;

  function release(transition) {
    if (!transition?.session || transition.released) return;
    transition.released = true;
    try { transition.session.release?.(); } catch { /* lease cleanup is best effort */ }
  }

  const safely = (effect) => {
    try { effect(); } catch { /* terminal cleanup must not strand ownership */ }
  };

  function cleanTransition(transition, detachAdopted = false) {
    if (!transition) return;
    if (detachAdopted && transition.phase === "confirming") safely(detach);
    release(transition);
  }

  function desired(transition) {
    return current(transition) && isDesired(transition.target);
  }

  function abandon(transition) {
    if (!current(transition)) return;
    pending = null;
    if (transition.phase === "confirming" && adopted?.sessionId === transition.session?.sessionId) {
      adopted = null;
      safely(detach);
    }
  }

  function mintMatchesTarget(transition, session) {
    return Boolean(session)
      && session.deviceId === transition.target.deviceId
      && desired(transition);
  }

  function beginConfirmation(transition, session) {
    transition.phase = "confirming";
    adopted = {
      deviceId: transition.target.deviceId,
      context: transition.target.context,
      sessionId: session.sessionId,
      carrier: transition.target.carrier,
      phase: "confirming",
    };
  }

  function failConfirmation(transition) {
    if (!current(transition)) return;
    pending = null;
    adopted = null;
    safely(detach);
  }

  async function confirm(transition, session) {
    try {
      await adopt(session, transition.target.carrier, () => desired(transition));
      if (!desired(transition)) return abandon(transition);
      adopted = { ...adopted, phase: "confirmed" };
      pending = null;
    } catch {
      failConfirmation(transition);
    } finally {
      release(transition);
    }
  }

  async function acceptMint(transition, session) {
    transition.session = session || null;
    if (!mintMatchesTarget(transition, session)) {
      abandon(transition);
      release(transition);
      return;
    }
    if (!transition.target.carrier) {
      transition.phase = "awaiting-carrier";
      release(transition);
      return;
    }
    beginConfirmation(transition, session);
    await confirm(transition, session);
  }

  async function run(transition) {
    if (!desired(transition)) return abandon(transition);
    try {
      await acceptMint(transition, await mint(transition.target.deviceId));
    } catch {
      abandon(transition);
      release(transition);
    }
  }

  function needsTransition(target) {
    return !adopted
      || adopted.deviceId !== target.deviceId
      || adopted.context !== target.context
      || adopted.phase !== "confirmed"
      || target.freshSession
      || (target.carrier && adopted.carrier !== target.carrier);
  }

  function beginTransition(target, operation) {
    const superseded = pending;
    const replacingCarrier = adopted?.deviceId === target.deviceId
      && adopted.context === target.context
      && adopted.carrier !== target.carrier;
    const mustDetach = superseded?.phase === "confirming" || target.freshSession || replacingCarrier;
    const transition = { target: { ...target }, phase: "minting", session: null, released: false };
    pending = transition;
    if (mustDetach) adopted = null;
    if (mustDetach) safely(detach);
    release(superseded);
    if (authority === operation) Promise.resolve().then(() => run(transition));
    return true;
  }

  function keepFollow(target, operation) {
    const superseded = pending;
    pending = null;
    adopted = { ...adopted, carrier: target.carrier };
    if (superseded?.phase === "confirming") safely(detach);
    if (authority === operation) safely(() => ride(target.carrier));
    release(superseded);
    return true;
  }

  function refuseFollow(target) {
    const superseded = pending;
    pending = null;
    const interruptedConfirmation = superseded?.phase === "confirming";
    if (interruptedConfirmation) adopted = null;
    const ownsAdopted = adopted?.deviceId === target.deviceId && adopted.context === target.context;
    if (ownsAdopted) {
      adopted = { ...adopted, carrier: null, phase: "disconnected" };
      safely(() => ride(null));
    }
    cleanTransition(superseded, interruptedConfirmation);
    return false;
  }

  function request(target) {
    if (target.canAnswer && sameTarget(pending?.target, target)) return true;
    const operation = {};
    authority = operation;
    if (!target.canAnswer) return refuseFollow(target);
    return needsTransition(target) ? beginTransition(target, operation) : keepFollow(target, operation);
  }

  function reset() {
    authority = {};
    const superseded = pending;
    const hadAdopted = Boolean(adopted);
    pending = null;
    adopted = null;
    if (hadAdopted) safely(detach);
    cleanTransition(superseded, false);
  }

  function snapshot() {
    if (pending) {
      const identity = {
        deviceId: pending.target.deviceId,
        sessionId: pending.session?.sessionId ?? null,
        carrier: pending.target.carrier,
      };
      return publicState(pending.phase, identity);
    }
    return adopted ? publicState(adopted.phase, adopted) : publicState("empty");
  }

  return { request, reset, snapshot };
}
