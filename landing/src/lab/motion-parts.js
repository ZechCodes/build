// The pieces the lab's chaotic variants (chaos.js) are built from: how a
// pill answers a passing wave, an arrival and the moment Build takes over,
// and the requests' way to their rows. Pure: each answers a part of a pose
// (posed.js) for a moment.
import { ATTENTION, random } from "../hero/field.js";
import { power2InOut, rippleHit, rippleReach } from "../hero/timing.js";

export const clamp01 = (value) => Math.min(1, Math.max(0, value));
export const easeOutCubic = (p) => 1 - (1 - clamp01(p)) ** 3;
const easeInQuad = (p) => clamp01(p) ** 2;

/** An overshooting ease: past the mark, then back onto it. */
export function easeOutBack(p, overshoot = 2.2) {
  const q = clamp01(p) - 1;
  return 1 + (overshoot + 1) * q ** 3 + overshoot * q ** 2;
}

/** A pill struck `tau` seconds ago, each part at unit strength: a shove out
 *  and back (`push`), a swell that rings (`swell`), a twist (`twist`) and a
 *  flash of light (`flash`), all dying away within about half a second. */
export function jolt(tau) {
  if (tau < 0) return { push: 0, swell: 0, twist: 0, flash: 0 };
  return {
    push: Math.exp(-tau / 0.16) * Math.sin((2 * Math.PI * tau) / 0.32),
    swell: Math.exp(-tau / 0.1) * Math.cos((2 * Math.PI * tau) / 0.2),
    twist: Math.exp(-tau / 0.18) * Math.sin((2 * Math.PI * tau) / 0.24),
    flash: Math.exp(-tau / 0.12),
  };
}

/** A shove that stays: from nothing to `1`, overshooting a little, as a
 *  pill knocked aside by a newcomer settles where it was pushed. */
export function shove(tau) {
  if (tau < 0) return 0;
  return 1 - Math.exp(-tau / 0.05) * Math.cos((2 * Math.PI * tau) / 0.2);
}

export function unitAway([x, y], [ox, oy]) {
  const distance = Math.hypot(x - ox, y - oy);
  return distance === 0 ? [0, 0] : [(x - ox) / distance, (y - oy) / distance];
}

/** When a wave leaving `origin` at `t0` and spreading at `speed` px/s
 *  reaches a pill that started at `[x, y]` and moves at `drift` px/s along
 *  x. The wave is always the faster, so it reaches it once. */
export function waveHit({ x, y, drift = 0 }, { origin: [ox, oy], t0, speed }) {
  const dx = x + drift * t0 - ox;
  const dy = y - oy;
  const a = drift ** 2 - speed ** 2;
  const b = 2 * dx * drift;
  const c = dx ** 2 + dy ** 2;
  return t0 + (-b - Math.sqrt(b ** 2 - 4 * a * c)) / (2 * a);
}

/** The field and the stage, as every variant reads them: its pills (each
 *  with its lane's speed), where Build takes over, and a seeded draw. */
export function chaosContext({ field, stage }, seed) {
  const narrow = field.width < 768;
  const pills = field.lanes.filter((lane) => lane.shown).flatMap((lane) => lane.pills.filter((pill) => pill.shown).map((pill) => ({ ...pill, speed: lane.speed })));
  return {
    field,
    stage,
    timing: stage.timing,
    unit: narrow ? 0.6 : 1,
    reach: rippleReach(stage.origin, field),
    diagonal: Math.hypot(field.width, field.height),
    next: random(seed),
    routine: pills.filter((pill) => !pill.attention),
    requests: ATTENTION.flatMap((entry, index) => pills.filter((pill) => pill.attention === entry.id).map((pill) => ({ pill, index }))),
    hiddenOf: (used) => pills.filter((pill) => !used.has(pill)).map((pill) => pill.element),
  };
}

/** Whether a pill is in sight at some moment before Build takes over. */
export function inPlay(pill, { field, timing }) {
  const until = timing.ripple[1];
  const [from, to] = [pill.x, pill.x + pill.speed * until].sort((a, b) => a - b);
  return to + pill.width / 2 > 0 && from - pill.width / 2 < field.width;
}

/** The wave Build takes over with, from the laptop's screen: where it
 *  reaches the pill (standing at `at` then), it blasts it outward, swollen
 *  and turning, gone within `fade` seconds. */
export function takeover(at, { stage, reach, timing, unit }, { distance, swell, turn, fade }) {
  const hit = rippleHit(at, stage.origin, reach, timing);
  const [ux, uy] = unitAway(at, stage.origin);
  return {
    hit,
    until: hit + fade,
    at(t) {
      const p = (t - hit) / fade;
      if (p <= 0) return null;
      const out = easeOutCubic(p * 1.4) * distance * unit;
      return { x: ux * out, y: uy * out, swell: swell * easeOutCubic(p), twist: turn * easeOutCubic(p), keep: 1 - easeInQuad(p) };
    },
  };
}

/** A request: it holds where it stands, breathing, so it can be read, then
 *  flies to its row as on the home page, shrinking and fading as it lands.
 *  Endless, it only breathes. */
export function requestPose({ pill, index }, { stage, timing }, { endless = false, appear = -1 } = {}) {
  const landing = timing.landings[index];
  const takeOff = landing - timing.flight;
  const [rx, ry] = stage.rows[index];
  const breathe = (t) => 1 + 0.05 * Math.sin(2 * Math.PI * 2 * t);
  return {
    pill,
    until: endless ? undefined : landing,
    z: 100_000,
    pose(t) {
      const shown = t < appear ? 0 : easeOutBack((t - appear) / 0.3);
      if (endless || t < takeOff) return { x: 0, y: 0, rotate: 0, scale: breathe(t) * Math.max(0.001, shown), opacity: clamp01(shown * 2) };
      const p = power2InOut(clamp01((t - takeOff) / timing.flight));
      return { x: (rx - pill.x) * p, y: (ry - pill.y) * p, rotate: 0, scale: 1 - 0.4 * p, opacity: p < 0.6 ? 1 : Math.max(0, 1 - (p - 0.6) / 0.4) };
    },
  };
}
