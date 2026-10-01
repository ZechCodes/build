// The lab's chaotic takes on the flood, for the user to point at (#311).
// Each is a set of poses for posed.js, built from motion-parts.js, from the
// same measured field as variant A:
//
//   B, burst: notifications fly in from every edge and pop where they land,
//     faster and faster, knocking the ones already there aside and piling
//     on top of them.
//   C, shockwave: the lanes drift as in A while rings of force cross the
//     field from all over, more and more often, shoving, swelling, twisting
//     and lighting each pill as they pass.
//   D, crescendo: all of it rising together: the lanes speed up, pills pop
//     in on top faster and faster, the shaking grows and the waves come
//     harder and closer together, until Build takes over.
//
// Every one ends as A does: one wave from the laptop's screen clears the
// field and the three requests fly to their rows. Endless, none resolves.
import { clamp01, chaosContext, easeOutBack, easeOutCubic, inPlay, jolt, requestPose, shove, takeover, unitAway, waveHit } from "./motion-parts.js";
import { posedMotion } from "./posed.js";

const between = (next, low, high) => low + (high - low) * next();
const pick = (next, list) => list[Math.floor(next() * list.length)];
const REST = Object.freeze({ x: 0, y: 0, rotate: 0, scale: 1, opacity: 1 });
// A pill not yet arrived: all but invisible and at a fair size, so the
// browser has drawn it before it is needed, not on the frame it appears.
const WAITING = Object.freeze({ opacity: 0.002, scale: 0.6 });

// Shuffled, then the first `count`.
function sampleOf(next, list, count) {
  const shuffled = list.map((item) => [next(), item]).sort(([a], [b]) => a - b).map(([, item]) => item);
  return shuffled.slice(0, count);
}

// Parts of a pose summed: offsets and turns add, swells add to a scale of
// one, and `keep` (what is left of it) multiplies its light.
function compose(base, parts) {
  const pose = { x: 0, y: 0, rotate: 0, scale: 1, opacity: base };
  let keep = 1;
  let flash = 0;
  for (const part of parts) {
    if (!part) continue;
    pose.x += part.x || 0;
    pose.y += part.y || 0;
    pose.rotate += part.twist || 0;
    pose.scale += part.swell || 0;
    flash += part.flash || 0;
    keep *= part.keep ?? 1;
  }
  pose.opacity = clamp01((base + (1 - base) * clamp01(flash)) * keep);
  pose.scale = Math.max(0.001, pose.scale);
  return pose;
}

function finish(context, { pills, endless, period }) {
  const requests = context.requests.map((request) => requestPose(request, context, { endless }));
  const used = new Set([...pills, ...requests].map((entry) => entry.pill));
  return posedMotion({
    pills: [...pills, ...requests].map(({ pill, ...motion }) => ({ element: pill.element, ...motion })),
    hidden: context.hiddenOf(used),
    end: endless ? Infinity : context.timing.settle[1],
    period,
  });
}

// --- waves (C and D) ----------------------------------------------------

// Waves from all over the field, closer together as `t0`s run on; their
// strength rises with `strength(t0)`.
function wavesOf(context, { from, until, gap, shrink, strength }) {
  const { field, next, diagonal } = context;
  const waves = [];
  for (let t0 = from, step = gap; t0 < until; t0 += step, step *= shrink) {
    waves.push({ t0, origin: [between(next, 0.08, 0.92) * field.width, between(next, 0.08, 0.92) * field.height], speed: diagonal / 0.6, strength: strength(t0) });
  }
  return waves;
}

// How a pill answers every wave: when each reaches it, from which way, and
// how hard, nearer waves harder.
function struckBy(pill, waves, context, drift = pill.speed) {
  const side = pick(context.next, [-1, 1]);
  return waves.map((wave) => {
    const hit = waveHit({ x: pill.x, y: pill.y, drift }, wave);
    const at = [pill.x + drift * hit, pill.y];
    const near = 1 - 0.5 * Math.min(1, Math.hypot(at[0] - wave.origin[0], at[1] - wave.origin[1]) / context.diagonal);
    return { hit, away: unitAway(at, wave.origin), force: wave.strength * near, side };
  });
}

function waveParts(t, strikes, { unit }, period) {
  return strikes.map(({ hit, away: [ux, uy], force, side }) => {
    const tau = period ? (((t - hit) % period) + period) % period : t - hit;
    const { push, swell, twist, flash } = jolt(tau);
    return { x: ux * push * 30 * unit * force, y: uy * push * 30 * unit * force, swell: 0.38 * swell * force, twist: side * 8 * twist * force, flash: 0.9 * flash * Math.min(1, force) };
  });
}

// --- B, burst -----------------------------------------------------------

// Where a pill bursts in from: beyond a random edge, level with where it
// lands, give or take.
function edgeStart(next, [tx, ty], { width, height }) {
  const edge = Math.floor(next() * 4);
  const along = (span, at) => at + (next() - 0.5) * span * 0.5;
  return [
    [-0.12 * width, along(height, ty)],
    [1.12 * width, along(height, ty)],
    [along(width, tx), -0.12 * height],
    [along(width, tx), 1.12 * height],
  ][edge];
}

function burstArrivals(context, { endless }) {
  const { field, next, routine, timing, unit } = context;
  const candidates = routine.filter((pill) => pill.x > -0.05 * field.width && pill.x < 1.05 * field.width);
  const chosen = sampleOf(next, candidates, unit < 1 ? 70 : 170);
  const window = endless ? BURST_PERIOD : timing.ripple[0];
  return chosen.map((pill) => {
    const target = [between(next, 0.03, 0.97) * field.width, between(next, 0.03, 0.97) * field.height];
    const already = !endless && next() < 0.22;
    const arrive = already ? -1 : window * (endless ? next() : Math.sqrt(next()) * 0.95);
    const flight = between(next, 0.18, 0.32);
    return { pill, target, start: edgeStart(next, target, field), arrive, landed: already ? -1 : arrive + flight, flight, turn: between(next, -30, 30), life: between(next, 1.4, 2.6), phase: next() * 6 };
  });
}

const BURST_PERIOD = 3.5;
const BURST_REACH = 200;

// The newcomers that land near a pill after it has, each pushing it aside.
function knocksOn(arrival, arrivals, { unit }, endless) {
  return arrivals.flatMap((other) => {
    if (other === arrival || (!endless && other.landed <= arrival.landed)) return [];
    const distance = Math.hypot(other.target[0] - arrival.target[0], other.target[1] - arrival.target[1]);
    if (distance > BURST_REACH * unit) return [];
    return [{ landed: other.landed, away: unitAway(arrival.target, other.target), force: 1 - distance / (BURST_REACH * unit) }];
  });
}

function burstPose(arrival, knocks, context, { endless, blast }) {
  const { pill, target, start, flight, turn, life, phase } = arrival;
  const settled = [target[0] - pill.x, target[1] - pill.y];
  const startAt = [start[0] - pill.x, start[1] - pill.y];
  const amount = 34 * context.unit;
  return (time) => {
    const t = endless ? (((time - arrival.arrive) % BURST_PERIOD) + BURST_PERIOD) % BURST_PERIOD + arrival.arrive : time;
    const since = t - arrival.arrive;
    if (arrival.arrive >= 0 && since < 0) return { ...REST, ...WAITING, x: startAt[0], y: startAt[1] };
    const p = arrival.arrive < 0 ? 1 : since / flight;
    if (p < 1) {
      const eased = easeOutBack(p, 1.4);
      return { x: startAt[0] + (settled[0] - startAt[0]) * eased, y: startAt[1] + (settled[1] - startAt[1]) * eased, rotate: turn * (1 - p), scale: WAITING.scale + (1 - WAITING.scale) * p, opacity: Math.max(WAITING.opacity, pill.opacity * clamp01(p * 3)) };
    }
    const sinceLanding = since - flight;
    const knocked = knocks.map(({ landed, away: [ux, uy], force }) => {
      const tau = endless ? (((time - landed) % BURST_PERIOD) + BURST_PERIOD) % BURST_PERIOD : t - landed;
      const moved = endless ? jolt(tau).push * 1.4 : shove(tau);
      return { x: ux * amount * force * moved, y: uy * amount * force * moved, twist: turn * 0.25 * force * jolt(tau).twist, swell: -0.12 * force * jolt(tau).swell };
    });
    const landing = arrival.arrive < 0 ? null : { swell: 0.3 * jolt(sinceLanding).swell, flash: 0.8 * jolt(sinceLanding).flash };
    const shake = { y: 1.5 * context.unit * Math.sin(2 * Math.PI * 9 * t + phase) };
    const leaving = endless ? popOut(sinceLanding - life) : blast?.at(time);
    return compose(pill.opacity, [{ x: settled[0], y: settled[1] }, landing, shake, ...knocked, leaving]);
  };
}

// Endless: a pill pops out of sight when its time is up, until it comes again.
function popOut(since) {
  if (since < 0) return null;
  const p = since / 0.18;
  return { swell: 0.3 * easeOutCubic(p), keep: 1 - clamp01(p) };
}

export function burstFlood({ field, stage }, { endless = false } = {}) {
  const context = chaosContext({ field, stage }, 311_2);
  const arrivals = burstArrivals(context, { endless });
  const order = [...arrivals].sort((a, b) => a.landed - b.landed);
  const pills = arrivals.map((arrival) => {
    const blast = endless ? null : takeover(arrival.target, context, { distance: between(context.next, 110, 190), swell: 0.25, turn: between(context.next, -18, 18), fade: context.timing.fade * 1.6 });
    return { pill: arrival.pill, pose: burstPose(arrival, knocksOn(arrival, arrivals, context, endless), context, { endless, blast }), until: blast?.until, z: order.indexOf(arrival) + 1 };
  });
  return finish(context, { pills, endless, period: BURST_PERIOD });
}

// --- C, shockwave ---------------------------------------------------------

const SHOCK_PERIOD = 3;

export function shockwaveFlood({ field, stage }, { endless = false } = {}) {
  const context = chaosContext({ field, stage }, 311_3);
  const { timing } = context;
  const waves = endless
    ? wavesOf(context, { from: 0, until: SHOCK_PERIOD, gap: SHOCK_PERIOD / 5, shrink: 1, strength: () => 1.5 })
    : wavesOf(context, { from: 0.02, until: timing.ripple[0] - 0.1, gap: 0.4, shrink: 0.78, strength: (t0) => 1 + t0 / timing.ripple[0] });
  // Endless, the lanes hold still, so each wave lands where it is seen.
  const routine = endless ? context.routine.filter((pill) => pill.x > -pill.width && pill.x < field.width + pill.width) : context.routine.filter((pill) => inPlay(pill, context));
  const pills = routine.map((pill) => {
    const drift = endless ? 0 : pill.speed;
    const strikes = struckBy(pill, waves, context, drift);
    const blast = endless ? null : takeover([pill.x + drift * timing.ripple[0], pill.y], context, { distance: between(context.next, 120, 200), swell: 0.4, turn: between(context.next, -20, 20), fade: timing.fade * 1.6 });
    return {
      pill,
      drift: endless ? undefined : { speed: drift },
      until: blast?.until,
      pose: (t) => compose(pill.opacity, [...waveParts(t, strikes, context, endless ? SHOCK_PERIOD : 0), blast?.at(t)]),
    };
  });
  return finish(context, { pills, endless, period: SHOCK_PERIOD });
}

// --- D, crescendo -----------------------------------------------------------

const CRESCENDO_PERIOD = 2.5;

// How far into the rise, 0 to 1, squared: slow at first, then a rush.
const intensityAt = (t, peak) => clamp01(t / peak) ** 2;

// The lanes' travel by `t` as they speed up to three times their pace at
// the peak, then hold it.
function warpedTravel(t, peak) {
  const rising = Math.min(t, peak);
  return rising + (2 * rising ** 3) / (3 * peak ** 2) + 3 * Math.max(0, t - peak);
}

// The shaking's phase: its frequency climbs from 5 Hz to 12 with the rise.
function shakePhase(t, peak) {
  const rising = Math.min(t, peak);
  return 2 * Math.PI * (5 * t + (7 * rising ** 3) / (3 * peak ** 2) + 7 * Math.max(0, t - peak));
}

function crescendoPose(pill, { context, peak, strikes, popAt, blast, endless, seed }) {
  const { unit } = context;
  const [phase, wobble, lean] = seed;
  return (t) => {
    const rise = endless ? 1 : intensityAt(t, peak);
    const travel = endless ? 0 : pill.speed * warpedTravel(t, peak);
    const shaking = endless ? 2 * Math.PI * 12 * t : shakePhase(t, peak);
    const amount = (1.5 + 9 * rise) * unit;
    const shake = { x: 0.6 * amount * Math.sin(shaking * wobble + phase), y: amount * Math.sin(shaking + phase), twist: lean * (0.5 + 6 * rise) * Math.sin(shaking * 0.7 + phase) };
    const since = endless ? (((t - popAt) % CRESCENDO_PERIOD) + CRESCENDO_PERIOD) % CRESCENDO_PERIOD : t - popAt;
    if (!endless && since < 0) return { ...REST, ...WAITING, x: travel };
    const pop = endless
      ? { swell: 0.35 * jolt(since).swell, flash: 0.6 * jolt(since).flash }
      : { swell: (WAITING.scale + (1 - WAITING.scale) * easeOutBack(since / 0.22, 2.6)) - 1, keep: Math.max(WAITING.opacity, clamp01(since / 0.06)) };
    return compose(pill.opacity, [{ x: travel }, shake, pop, ...waveParts(t, strikes, context, endless ? CRESCENDO_PERIOD : 0), blast?.at(t)]);
  };
}

export function crescendoFlood({ field, stage }, { endless = false } = {}) {
  const context = chaosContext({ field, stage }, 311_4);
  const { timing, next } = context;
  const peak = timing.ripple[0];
  const waves = endless
    ? wavesOf(context, { from: 0, until: CRESCENDO_PERIOD, gap: CRESCENDO_PERIOD / 5, shrink: 1, strength: () => 2 })
    : wavesOf(context, { from: 0.35 * peak, until: peak - 0.05, gap: 0.24 * peak, shrink: 0.72, strength: (t0) => 0.5 + 1.5 * intensityAt(t0, peak) });
  const routine = endless ? context.routine.filter((pill) => pill.x > -pill.width && pill.x < field.width + pill.width) : context.routine.filter((pill) => inPlay(pill, context));
  const pills = routine.map((pill) => {
    const popAt = endless ? next() * CRESCENDO_PERIOD : next() < 0.45 ? -1 : peak * Math.sqrt(next()) * 0.95;
    // Its pace at the peak, for when the waves reach it.
    const strikes = struckBy(pill, waves, context, endless ? 0 : pill.speed * 2);
    const at = [pill.x + (endless ? 0 : pill.speed * warpedTravel(peak, peak)), pill.y];
    const blast = endless ? null : takeover(at, context, { distance: between(next, 160, 260), swell: 0.5, turn: between(next, -28, 28), fade: timing.fade * 1.6 });
    // Wobbles that fit the endless period a whole number of times.
    const seed = [next() * 2 * Math.PI, pick(next, [0.8, 0.9, 1.1, 1.2]), pick(next, [-1, 1])];
    return { pill, until: blast?.until, z: Math.round((popAt + 2) * 1000), pose: crescendoPose(pill, { context, peak, strikes, popAt, blast, endless, seed }) };
  });
  return finish(context, { pills, endless, period: CRESCENDO_PERIOD });
}
