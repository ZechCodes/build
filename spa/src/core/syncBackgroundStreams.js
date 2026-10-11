// A device's background checkout phase has exactly two streams. Each stream
// owns an entity's ordered reads and cache writes before taking another one.
// Foreground work never acquires a stream; it uses the phase planner's barrier.

/** The owning pass supplies its captured lifetime, so abandoned RPCs cannot
 *  keep a later session's streams occupied. A failed stream waits for its
 *  sibling to finish before the pass can hand off to a queued refresh. */
export async function drainBackgroundStreams({ items, active, read }) {
  const queue = items[Symbol.iterator]();
  let failed = false;
  const drain = async () => {
    let recovered = true;
    try {
      while (active() && !failed) {
        const next = queue.next();
        if (next.done) return recovered;
        const refreshed = await read(next.value);
        recovered = refreshed && recovered;
      }
      return false;
    } catch (error) {
      failed = true;
      throw error;
    }
  };
  const streams = await Promise.allSettled([drain(), drain()]);
  const failure = streams.find((stream) => stream.status === "rejected");
  if (failure) throw failure.reason;
  return active() && streams.every((stream) => stream.value);
}
