// The hero entrance's clock and the maths its tweens are built from. Pure:
// entrance.js turns these numbers into one GSAP timeline.

// Seconds. One timeline, these phases; the laptop comes in while the ripple
// is still passing. `fade` is how long a pill takes to go once the wave
// reaches it, `decel` how long it takes to stop, `flight` how long an
// attention request takes to reach its row, and `landings` when each of the
// three arrives, in ATTENTION's order.
export const HERO_TIMING = Object.freeze({
  field: [0, 1.2],
  ripple: [1.2, 2],
  laptop: [1.6, 3.2],
  converge: [2.2, 3],
  message: [3, 3.5],
  settle: [3.5, 4],
  fade: 0.28,
  decel: 0.5,
  flight: 0.55,
  landings: [2.75, 2.88, 3],
});

// A phone tells the same story a little faster.
export const NARROW_TIMING = Object.freeze({
  field: [0, 1],
  ripple: [1, 1.7],
  laptop: [1.35, 2.8],
  converge: [1.9, 2.65],
  message: [2.65, 3.1],
  settle: [3.1, 3.6],
  fade: 0.25,
  decel: 0.45,
  flight: 0.5,
  landings: [2.4, 2.52, 2.65],
});

const PHASES = ["field", "ripple", "converge", "message", "settle"];

/** The phase a moment belongs to, latest first where phases overlap. */
export function phaseAt(time, timing) {
  if (time >= timing.settle[1]) return "settled";
  return [...PHASES].reverse().find((phase) => time >= timing[phase][0]) || "field";
}

function clamp(value, low = 0, high = 1) {
  return Math.min(high, Math.max(low, value));
}

function smoothstep(value) {
  const bounded = clamp(value);
  return bounded * bounded * (3 - 2 * bounded);
}

// Exact at both ends, so a finished tween lands on its numbers.
const mix = (from, to, amount) => from * (1 - amount) + to * amount;

/** How far the wave travels: to the field's farthest corner. */
export function rippleReach([x, y], { width, height }) {
  return Math.max(...[[0, 0], [width, 0], [0, height], [width, height]].map(([cx, cy]) => Math.hypot(cx - x, cy - y)));
}

/** When the wave reaches a point: by its distance from the origin alone, so
 *  the field fades as a ring spreading from the screen, and the last pill
 *  has faded by the ripple's end. */
export function rippleHit([x, y], [ox, oy], reach, timing) {
  const [start, end] = timing.ripple;
  return start + (Math.hypot(x - ox, y - oy) / reach) * (end - start - timing.fade);
}

/** Whether a pill moving at `speed` px/s (negative runs left) is in the
 *  field at some moment of the next `seconds`: in sight already, or near
 *  enough the side its lane comes from to arrive. */
export function reachesField({ x, width }, speed, fieldWidth, seconds) {
  const [from, to] = [x, x + speed * seconds].sort((a, b) => a - b);
  return to + width / 2 > 0 && from - width / 2 < fieldWidth;
}

/** A small push away from the origin, `amount` px long. */
export function outward([x, y], [ox, oy], amount) {
  const distance = Math.hypot(x - ox, y - oy);
  if (distance === 0) return [0, 0];
  return [((x - ox) / distance) * amount, ((y - oy) / distance) * amount];
}

/** How far a pill moving at `speed` px/s travels while power1.out brakes it
 *  to a stop over `seconds`: that ease starts at twice its average speed. */
export function decelDistance(speed, seconds) {
  return (speed * seconds) / 2;
}

// GSAP's power1 eases, so the field brakes and fades as its tweens did.
const power1Out = (p) => 1 - (1 - p) ** 2;
const power1In = (p) => p * p;

/** A pill's way through the ripple, from where it was measured at `start`:
 *  it moves with its lane at `speed` px/s until the wave reaches it, then
 *  brakes to a stop pushed a little outward, fading as it does. `at(time)`
 *  is its shift from the measured spot and how far it has faded (0 to 1). */
export function pillPath({ x, y }, speed, { origin, reach, timing, start, nudge }) {
  const begin = timing.ripple[0];
  const atBegin = [x + speed * (begin - start), y];
  // One coming in from beyond the wave's reach still fades in time.
  const hit = Math.min(rippleHit(atBegin, origin, reach, timing), timing.ripple[1] - timing.fade);
  const [pushX, pushY] = outward(atBegin, origin, nudge);
  const braking = decelDistance(speed, timing.decel);
  const carried = speed * (hit - start);
  return {
    hit,
    at(time) {
      if (time <= hit) return { dx: speed * (time - start), dy: 0, faded: 0 };
      const eased = power1Out(clamp((time - hit) / timing.decel));
      return { dx: carried + (braking + pushX) * eased, dy: pushY * eased, faded: power1In(clamp((time - hit) / timing.fade)) };
    },
  };
}

export { LANDED_SCALE, power2InOut, requestFlight, laptopEntrancePose, laptopRevealPose, posterReveal, revealEase } from "../../hero/timing.js";
