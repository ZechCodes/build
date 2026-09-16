// A tiny status fan-out for the shared terminal socket. The socket reports one
// status at a time ('connecting'|'connected'|'disconnected') through a single
// onStatus slot; this multiplexes it to every mounted pane's overlay.
//
// Pure: no DOM, no socket references. `subscribe` immediately delivers the last
// known status (nothing until the first `set`, so a pane that mounts before the
// socket has ever connected shows no chip rather than a false "disconnected").

export function createStatusHub() {
  let current = null;
  const subscribers = new Set();
  return {
    set(status) {
      current = status;
      for (const subscriber of subscribers) subscriber(status);
    },
    clear() {
      current = null;
    },
    subscribe(fn) {
      subscribers.add(fn);
      if (current !== null) fn(current);
      return () => subscribers.delete(fn);
    },
  };
}
