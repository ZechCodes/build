// Which carrier one session rides, and what runs on every change.
//
// The browser end of the teardown rule the bridge's SessionRegistry owns: a
// session ends when its last carrier is gone, or when its client says so. A
// relay socket loss, a DataChannel close and an upgrade are each one slot
// changing here, and migration in either direction is the one code path below.

/** The rule, stated once: a peer carrier carries while it is there, the relay
 *  carries otherwise, and nothing carrying is the end of the session. */
const carrying = (relay, peer) => peer ?? relay;

/**
 * @param session an object with `rideOn(carrier)` — the wire it sends on now,
 *   `null` when nothing is carrying.
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
      return settle();
    },
    peer(carrier) {
      peer = carrier ?? null;
      return settle();
    },
    active: () => active,
    /** The client said so: no later carrier loss is this session's end. */
    close() {
      closed = true;
      relay = null;
      peer = null;
      active = null;
    },
  };
}
