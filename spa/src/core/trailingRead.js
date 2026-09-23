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

/**
 * Wrap `read(key)` so that, per key, at most one call is out at a time.
 *
 * Asking while that key's read is out returns the read already out and marks
 * the key for one more run when it settles, however many times it was asked.
 * A read that threw still takes its trailing run.
 */
export function trailingRead(read) {
  const out = new Map();
  const run = (key) => {
    const held = { again: false, done: null };
    out.set(key, held);
    let started;
    try {
      started = Promise.resolve(read(key));
    } catch (error) {
      started = Promise.reject(error);
    }
    held.done = started.finally(() => {
      if (held.again) void run(key).catch(() => {});
      else out.delete(key);
    });
    return held.done;
  };
  return (key = "") => {
    const held = out.get(key);
    if (!held) return run(key);
    held.again = true;
    return held.done;
  };
}
