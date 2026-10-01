// The lab's clock: the moment the field is shown at, advanced each frame by
// the seconds that have passed at the chosen speed. At the end of the beat
// it stops, or starts again when it loops; in endless mode the flood never
// resolves, so there is no end.

/** The scrubber's span in endless mode, in seconds. */
export const ENDLESS_WINDOW = 30;

export function advance(clock, seconds, end) {
  if (!clock.playing) return clock;
  const step = seconds * clock.speed;
  if (clock.endless) return { ...clock, time: clock.time + step };
  // Played again once finished: from the top.
  if (clock.time >= end) return { ...clock, time: Math.min(step, end) };
  const time = clock.time + step;
  if (time < end) return { ...clock, time };
  return clock.loop ? { ...clock, time: (time - end) % end } : { ...clock, time: end, playing: false };
}
