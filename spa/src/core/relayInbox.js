// The relay socket's messages, as something a handshake can wait on.
//
// Both session modules open a socket, authenticate on it, and then wait for the
// two messages their handshake needs while everything else goes past. That wait
// is this: one queue, one waiter list, one deadline, and a socket close that
// answers whoever is waiting with nothing rather than leaving them hanging.

/**
 * @param socket an authenticated-or-not relay socket. The inbox never writes to
 *   it and never closes it — it only reads what arrives.
 * @param observe every message, before anyone waiting on it sees it. The
 *   caller's device store is kept current whether or not a handshake is
 *   listening, which is the one thing that must not depend on who is waiting.
 */
export function relayInbox(socket, observe = () => {}) {
  const queue = [];
  const waiters = [];
  let closed = false;

  socket.addEventListener("message", (event) => {
    let message;
    try {
      message = JSON.parse(typeof event.data === "string" ? event.data : event.data.toString());
    } catch {
      return; // the relay speaks JSON; anything else is not a message we have
    }
    observe(message);
    if (waiters.length) waiters.shift()(message);
    else queue.push(message);
  });
  socket.addEventListener("close", () => {
    closed = true;
    waiters.splice(0).forEach((answer) => answer(null));
  });

  const next = () =>
    new Promise((resolve) => {
      if (queue.length) return resolve(queue.shift());
      if (closed) return resolve(null);
      waiters.push(resolve);
    });

  return {
    /**
     * The next message `predicate` takes, `null` once the socket has closed
     * without one, or a rejection when `timeoutMs` passes first. Everything the
     * predicate refuses is dropped: nobody else is reading this socket.
     */
    async matching(predicate, timeoutMs = 0, timeoutMessage = "the relay went quiet") {
      let deadline;
      const expiry = timeoutMs
        ? new Promise((_, reject) => (deadline = setTimeout(() => reject(new Error(timeoutMessage)), timeoutMs)))
        : null;
      try {
        for (;;) {
          const message = await (expiry ? Promise.race([next(), expiry]) : next());
          if (message === null || predicate(message)) return message;
        }
      } finally {
        clearTimeout(deadline);
      }
    },
  };
}
