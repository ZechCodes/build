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
 * Asking while that key's read is out marks the key for one more run when it
 * settles, however many times it was asked, and returns that trailing run's
 * promise: a folded ask settles with the read begun after it, never with the
 * one already out, so a caller that wrote and then asked paints what it wrote
 * (#126). A read that threw still takes its trailing run. `generationOf(key)`
 * names the session a read belongs to; asking under a different one starts a
 * read of its own, which is also what an ask folded under the older session
 * settles with, and the older read settling afterwards starts nothing.
 */
export function trailingRead(read, { generationOf = () => null } = {}) {
  const out = new Map();
  const run = (key, generation) => {
    const held = { again: false, done: null, next: null, generation };
    out.set(key, held);
    let started;
    try {
      started = Promise.resolve(read(key));
    } catch (error) {
      started = Promise.reject(error);
    }
    held.done = started.finally(() => {
      if (out.get(key) !== held) return; // overtaken by a newer session's read
      if (held.again) {
        held.next = run(key, generationOf(key));
        held.next.catch(() => {});
      } else out.delete(key);
    });
    return held.done;
  };
  return (key = "") => {
    const generation = generationOf(key);
    const held = out.get(key);
    if (!held) return run(key, generation);
    if (held.generation !== generation) return (held.next = run(key, generation));
    held.again = true;
    const next = () => held.next;
    return held.done.then(next, next);
  };
}
