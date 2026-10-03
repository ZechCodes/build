// Shared request flight and laptop reveal math for the home entrance.
const clamp = (value, low = 0, high = 1) => Math.min(high, Math.max(low, value));
const smoothstep = (value) => { const bounded = clamp(value); return bounded * bounded * (3 - 2 * bounded); };
const mix = (from, to, amount) => from * (1 - amount) + to * amount;

const PHASES = ["field", "ripple", "converge", "message", "settle"];
export function phaseAt(time, timing) {
  if (time >= timing.settle[1]) return "settled";
  return [...PHASES].reverse().find((phase) => time >= timing[phase][0]) || "field";
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
