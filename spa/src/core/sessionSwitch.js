// Which carrier one session rides, and what runs on every change.
//
// There is one carrier: the peer connection. The relay is a rendezvous, not a
// data plane (spec rules 1 and 4), so its slot here is `signaling` — the wire
// `rtc.*` rides while something is negotiating — and a session with no peer is
// a session nothing is carrying, whether or not a rendezvous is open.
//
// The browser end of the teardown rule the bridge's SessionRegistry owns: a
// session ends when its last carrier is gone, or when its client says so.

/** Signaling, and the one place that word is spelled. `rtc.*` never rides the
 *  channel it negotiates (spec §Signaling), and it is not the user's traffic,
 *  so an offline pause does not hold it back either. */
export const isSignaling = (method) => method.startsWith("rtc.");

/**
 * @param session an object with `rideOn(carrier)` — the wire it sends on now,
 *   `null` when nothing is carrying — and `readFrom(carrier)`, every wire it
 *   holds, sending or not.
 * @param onActive what re-establishes this session on the wire it just took
 *   (`session.hello`, a terminal re-attach). Whatever it returns is handed
 *   back, so a caller can wait for it.
 * @param onIdle the session's carrier is gone.
 */
export function createSessionSwitch({ session, onActive = () => {}, onIdle = () => {} }) {
  const slots = { signaling: null, peer: null };
  // Calls made before the wire they belong on was there. Held rather than
  // refused: the upgrade is in flight, and what fails them is the upgrade
  // failing (`fail`), not the moment they were asked.
  const queued = { signaling: [], peer: [] };
  let active = null;
  let closed = false;
  // Why there will be no wire, once somebody has said so. Latched: a session
  // whose upgrade failed does not hold the next call for a channel that is not
  // coming either.
  let failure = null;

  const answer = (slot) => queued[slot].splice(0).forEach(({ resolve }) => resolve(slots[slot]));

  const settle = () => {
    if (closed || slots.peer === active) return undefined;
    active = slots.peer;
    session.rideOn(active);
    return active ? onActive() : onIdle();
  };

  const take = (slot, carrier) => {
    slots[slot] = carrier ?? null;
    if (!slots[slot]) return undefined;
    // Read from it whether or not it carries: the rendezvous carries this
    // session's signaling answers, and they still have to arrive.
    session.readFrom(slots[slot]);
    answer(slot);
    return undefined;
  };

  return {
    /** The wire `rtc.*` rides while the rendezvous is open, `null` once it is
     *  closed. It never carries the session. */
    signaling(carrier) {
      return take("signaling", carrier);
    },

    /** The DataChannel this session rides, or `null` when it has none.
     *
     *  The one thing that lifts the latch: a session failed closed is failed
     *  closed until something is CARRYING it. The rendezvous re-attaching
     *  mid-restart is not that — it carries `rtc.*` and nothing else (rule 1) —
     *  and a user's call let back into the queue on it would wait out its whole
     *  deadline instead of being refused in the blocked device's own words. */
    peer(carrier) {
      if (carrier) failure = null;
      take("peer", carrier);
      return settle();
    },

    active: () => active,

    /**
     * Which wire one call rides: the rendezvous for signaling, the peer for
     * everything else. The one routing rule, in the one place that knows both
     * slots.
     *
     * A call made while its wire is not there WAITS for it: a user's call
     * during the upgrade rides the channel it is waiting for rather than the
     * relay, and an ICE restart asked for after the rendezvous closed waits for
     * the caller to reopen it. The wait is the caller's own timeout; `fail`
     * ends it when the wire is not coming, and a session its client has closed
     * answers it with nothing.
     */
    wireFor(method) {
      const slot = isSignaling(method) ? "signaling" : "peer";
      if (slots[slot] || closed) return slots[slot];
      if (failure) return Promise.reject(failure);
      return new Promise((resolve, reject) => queued[slot].push({ resolve, reject }));
    },

    /** The wire nothing was waiting on is not coming: this device is blocked.
     *  Everything held for it is refused in those words. */
    fail(error) {
      failure = error;
      for (const slot of Object.keys(queued)) queued[slot].splice(0).forEach(({ reject }) => reject(error));
    },

    /** The client said so: no later carrier loss is this session's end. */
    close() {
      closed = true;
      failure = null; // a client that closed a session is told nothing more about it
      slots.signaling = null;
      slots.peer = null;
      active = null;
      answer("signaling");
      answer("peer");
    },
  };
}
