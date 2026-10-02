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

// On a short phone the three requests start closer together vertically.
// Sending the bottom one first keeps each flight clear of the requests still
// waiting in the field. The laptop arrives on the same narrow-phone clock.
// The landings array remains indexed by [review, approval, question].
export const SHORT_NARROW_TIMING = Object.freeze({
  ...NARROW_TIMING,
  converge: [1.9, 3.01],
  message: [3.05, 3.5],
  settle: [3.5, 4],
  flight: 0.3,
  landings: [3.01, 2.68, 2.35],
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
 *  brakes to a stop, optionally pushed outward in the lab. `at(time)`
 *  is its shift from the measured spot and how far it has faded (0 to 1). */
export function pillPath({ x, y }, speed, { origin, reach, timing, start, push = 0 }) {
  const begin = timing.ripple[0];
  const atBegin = [x + speed * (begin - start), y];
  // One coming in from beyond the wave's reach still fades in time.
  const hit = Math.min(rippleHit(atBegin, origin, reach, timing), timing.ripple[1] - timing.fade);
  const [pushX, pushY] = push === 0 ? [0, 0] : outward(atBegin, origin, push);
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

// A landing request ends at this share of its size, as a row's height.
export const LANDED_SCALE = 0.6;

/** GSAP's power2.inOut, for a flight run off GSAP's clock (the lab page). */
export function power2InOut(p) {
  return p < 0.5 ? 4 * p ** 3 : 1 - (-2 * p + 2) ** 3 / 2;
}

/** A request `p` of the way (already eased) from where it stopped to its
 *  row: `base` is its offset at take-off, `from` its centre then and `to`
 *  the landing point. It shrinks to a row's size and fades over the last
 *  stretch. */
export function requestFlight(p, { base, from, to }) {
  return {
    transform: `translate(${base[0] + (to[0] - from[0]) * p}px, ${base[1] + (to[1] - from[1]) * p}px) scale(${1 + (LANDED_SCALE - 1) * p})`,
    opacity: p < 0.6 ? 1 : Math.max(0, 1 - (p - 0.6) / 0.4),
  };
}

// The laptop's way in: a controlled quarter turn from the right, the lid
// part way open, rising a little. Less of everything on a phone.
const ENTRANCE = Object.freeze({
  wide: Object.freeze({ yaw: 75, x: 5, y: 3, scale: 0.9, pitch: 4, lidOpen: 0.3 }),
  narrow: Object.freeze({ yaw: 35, x: 3, y: 2, scale: 0.94, pitch: 2, lidOpen: 0.3 }),
});

export function laptopEntrancePose(final, { narrow = false } = {}) {
  const offset = narrow ? ENTRANCE.narrow : ENTRANCE.wide;
  return {
    ...final,
    x: final.x + offset.x,
    y: final.y + offset.y,
    w: final.w * offset.scale,
    yaw: final.yaw + offset.yaw,
    pitch: final.pitch + offset.pitch,
    lidOpen: offset.lidOpen,
    opacity: 0,
  };
}

/** The pose `t` of the way in (t already eased by the timeline): in sight
 *  within the first third, the lid opening behind the turn. */
export function laptopRevealPose(t, from, to) {
  const pose = {};
  for (const key of Object.keys(to)) pose[key] = mix(from[key], to[key], t);
  pose.opacity = mix(from.opacity, to.opacity, smoothstep(t / 0.3));
  pose.lidOpen = mix(from.lidOpen, to.lidOpen, smoothstep((t - 0.15) / 0.75));
  return pose;
}

/** The poster's version of the same arc: a turn about its vertical axis in
 *  degrees, a shift right in vw, a scale and an opacity. */
export function posterReveal(t, { narrow = false } = {}) {
  const amount = clamp(t);
  return {
    turn: (narrow ? 12 : 28) * (1 - amount),
    shift: (narrow ? 3 : 7) * (1 - amount),
    scale: mix(0.93, 1, amount),
    opacity: smoothstep(amount / 0.3),
  };
}

// The laptop's ease: it gathers speed smoothly for the first quarter, then
// decelerates with confidence (cubic) onto its rest, with no jump in speed
// where the two meet.
const EASE_SPLIT = 0.25;
const EASE_OUT = 1 / ((1 - EASE_SPLIT) ** 2 * (1 + EASE_SPLIT / 2));
const EASE_IN = (3 * EASE_OUT * (1 - EASE_SPLIT) ** 2) / (2 * EASE_SPLIT);

export function revealEase(progress) {
  const p = clamp(progress);
  return p < EASE_SPLIT ? EASE_IN * p * p : 1 - EASE_OUT * (1 - p) ** 3;
}
