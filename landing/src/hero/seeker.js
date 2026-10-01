// Many compositor animations on one clock: each seeked to the clock's
// moment as flood.js's seek does, but an animation that has run out and
// holds its end is left alone until the clock goes back, so a held field
// is not restyled pill by pill on every tick.
import { seek } from "./flood.js";

/** `timed`: [animation, ms it runs before it holds (Infinity: never)]. */
export function seekAll(timed) {
  const held = new Set();
  return function update(time, playing, rate) {
    const ms = time * 1000;
    for (const [animation, runs] of timed) {
      if (ms >= runs && (held.has(animation) || animation.playState === "finished")) {
        // Run out on its own, it already holds its end.
        held.add(animation);
        continue;
      }
      // One the compositor has not started yet holds its time; seeking it
      // again would only put its start off further.
      if (playing && animation.pending) continue;
      seek(animation, ms, playing, runs, rate);
      if (ms >= runs) held.add(animation);
      else held.delete(animation);
    }
  };
}
