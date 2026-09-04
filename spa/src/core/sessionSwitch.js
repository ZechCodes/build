// Which carrier one session rides, and what runs on every change.
//
// The browser end of the teardown rule the bridge's SessionRegistry owns: a
// session ends when its last carrier is gone, or when its client says so. A
// relay socket loss, a DataChannel close and an upgrade are each one slot
// changing here, and migration in either direction is the one code path below.

/** The rule, stated once: a peer carrier carries while it is there, the relay
 *  carries otherwise, and nothing carrying is the end of the session. */
const carrying = (relay, peer) => peer ?? relay;

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
 * @param onIdle the session's last carrier is gone.
 */
export function createSessionSwitch({ session, onActive = () => {}, onIdle = () => {} }) {
  let relay = null;
  let peer = null;
  let active = null;
  let closed = false;
  const waitingForRelay = [];

  const answerRelayWaiters = () => waitingForRelay.splice(0).forEach((answer) => answer(relay));

  const settle = () => {
    const next = carrying(relay, peer);
    if (closed || next === active) return undefined;
    active = next;
    session.rideOn(next);
    return next ? onActive() : onIdle();
  };

  return {
    relay(carrier) {
      relay = carrier ?? null;
      if (relay) {
        // Read from it whether or not it ends up carrying: a relay that
        // re-attaches under a live channel carries this session's signaling
        // answers and nothing else, and they still have to arrive.
        session.readFrom(relay);
        answerRelayWaiters();
      }
      return settle();
    },
    peer(carrier) {
      peer = carrier ?? null;
      if (peer) session.readFrom(peer);
      return settle();
    },
    active: () => active,
    /**
     * Which wire one call rides: the relay for signaling, whatever is active
     * for everything else. The one routing rule, in the one place that knows
     * both slots.
     *
     * A signaling call made while the relay is detached WAITS for the
     * re-attach rather than failing: the link is already reconnecting, and an
     * ICE restart asked for in that window is exactly what policy 6 keeps the
     * session alive for. The wait is the caller's own timeout, and a session
     * its client has closed answers it with nothing.
     */
    wireFor(method) {
      if (!isSignaling(method)) return active;
      if (relay || closed) return relay;
      return new Promise((resolve) => waitingForRelay.push(resolve));
    },
    /** The client said so: no later carrier loss is this session's end. */
    close() {
      closed = true;
      relay = null;
      peer = null;
      active = null;
      answerRelayWaiters();
    },
  };
}
