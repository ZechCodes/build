// Endless mode for a drifting field: each lane's pills run round a loop
// longer than the field, wrapping out of sight, each on one infinite Web
// Animation, so the flood never runs dry and never resolves. Pure: the lab
// player turns these into animations.

// The space after a lane's last pill before its first comes round, in px.
const GAP = 24;

const leftOf = (pill) => pill.x - pill.width / 2;

/** The loop a lane's pills run round: from `start` (out of sight on the
 *  left) for `length` px, past the last pill and clear of the field. */
export function laneLoop({ pills }, fieldWidth) {
  const widest = Math.max(...pills.map((pill) => pill.width));
  const start = Math.min(...pills.map(leftOf), -widest);
  const end = Math.max(...pills.map((pill) => leftOf(pill) + pill.width + GAP), fieldWidth + widest);
  return { start, length: end - start };
}

/** One pill's way round its lane's loop at `speed` px/s (negative runs
 *  left): a translateX from `from` to `to` px over `duration` ms, begun
 *  `iterationStart` of the way through so it starts where it stands. */
export function pillLoop(pill, speed, { start, length }) {
  if (speed === 0) return null;
  const left = leftOf(pill);
  const phase = (left - start) / length;
  const duration = (length / Math.abs(speed)) * 1000;
  if (speed > 0) return { from: start - left, to: start + length - left, duration, iterationStart: phase };
  return { from: start + length - left, to: start - left, duration, iterationStart: (1 - phase) % 1 };
}
