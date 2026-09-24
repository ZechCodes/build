// One read out at a time, and one more after it when something asked meanwhile.
//
// A push says "this moved, read it again", and a busy project pushes every
// flush. A surface that answered each one with a fresh read — and kept only the
// newest answer — starved itself whenever a read took longer than the gap
// between pushes: every answer was overtaken before it landed, and every one
// of them still crossed the wire (#119, a Done issue still under "Needs you"
// for as long as the agents kept commenting). Here a read asked for while the
// same read is out is folded into ONE read after it, which starts once the
// current answer has landed, so every answer lands and the last word is always
// read after the last push.
//
// Folding is per session. A read out on a session that has since died may not
// answer for a long time, and the new session's greeting asks for everything
// again: that read starts at once rather than queueing behind the dead one.

/**
 * Wrap `read(key)` so that, per key and per session, at most one call is out
 * at a time.
 *
 * Asking while that key's read is out returns the read already out and marks
 * the key for one more run when it settles, however many times it was asked.
 * A read that threw still takes its trailing run. `generationOf(key)` names
 * the session a read belongs to; asking under a different one starts a read of
 * its own, and the older read settling afterwards starts nothing.
 */
export function trailingRead(read, { generationOf = () => null } = {}) {
  const out = new Map();
  const run = (key, generation) => {
    const held = { again: false, done: null, generation };
    out.set(key, held);
    let started;
    try {
      started = Promise.resolve(read(key));
    } catch (error) {
      started = Promise.reject(error);
    }
    held.done = started.finally(() => {
      if (out.get(key) !== held) return; // overtaken by a newer session's read
      if (held.again) void run(key, generationOf(key)).catch(() => {});
      else out.delete(key);
    });
    return held.done;
  };
  return (key = "") => {
    const generation = generationOf(key);
    const held = out.get(key);
    if (!held || held.generation !== generation) return run(key, generation);
    held.again = true;
    return held.done;
  };
}
