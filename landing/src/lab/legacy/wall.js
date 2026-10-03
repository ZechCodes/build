// The notification wall (#316), as data: where its slots stand, how each
// cycles its notifications, and the keyframes that play them. Pure, like
// field.js and timing.js: wall-motion.js puts it on the page.
//
// A slot holds two notifications that take turns: one slides in, shows for
// a moment and blurs out while the other slides in. Every slot keeps its
// own pace and phase, so at any moment the wall shows all three states
// everywhere. Its shape is the whole field, or a dome hanging from the top
// edge that thins and fades towards its rim and stays above the headline
// and the laptop; capped on a wide screen, so the dome holds about as many
// pills at 2560 as at 1440. At its exit (finite), a slot takes no new
// notification and blurs out what it shows.
import { ROUTINE_EVENTS, SUPPORTED_HARNESSES, random } from "./field.js";

export const NOTE_STATES = Object.freeze(["in", "shown", "out"]);
// Slide in decelerating; blur out accelerating.
export const WALL_EASE = Object.freeze({ in: "cubic-bezier(0.2, 0.8, 0.3, 1)", out: "cubic-bezier(0.5, 0, 0.75, 0.6)" });
// How far a blurred-out notification has gone, in px of blur.
const BLUR_PX = 7;

// The dome's half-width and depth on a wide screen at most, in px, and the
// share of its radius that stays solid before it fades.
const DOME = Object.freeze({
  wide: Object.freeze({ rx: 760, ry: 260, inner: 0.55 }),
  narrow: Object.freeze({ widthShare: 0.62, inner: 0.65 }),
  fade: 0.95,
});

// A slot's type, as a share of hero.css's: smaller on a phone, where the
// wall has a narrow band above the headline to fill.
const SIZES = Object.freeze({ wide: Object.freeze([0.86, 1.06]), narrow: Object.freeze([0.7, 0.9]) });

const between = (next, low, high) => low + (high - low) * next();
const round = (value, places = 1) => Number(value.toFixed(places));

/** The wall's outline: `kind` "full" is the whole field; "dome" is a
 *  half-ellipse centred on the top edge, as deep as `clear` (the top of the
 *  headline or the laptop, whichever is higher) allows. Past `inner` of its
 *  radius it thins and fades, and past `fade` there is nothing. */
export function wallShape({ width, narrow, clear, kind }) {
  if (kind === "full") return { kind, cx: width / 2 };
  const rx = narrow ? width * DOME.narrow.widthShare : Math.min(width / 2, DOME.wide.rx);
  const ry = narrow ? clear : Math.min(clear, DOME.wide.ry);
  return { kind, cx: width / 2, rx, ry, inner: narrow ? DOME.narrow.inner : DOME.wide.inner, fade: DOME.fade };
}

/** How far out in the dome a point is: 0 at the top centre, 1 on its rim. */
export const domeReach = (shape, [x, y]) => Math.hypot((x - shape.cx) / shape.rx, y / shape.ry);

// The share of slots kept at a reach: all of them in the solid middle,
// fewer and fewer towards the rim.
function density(shape, reach) {
  if (reach < shape.inner) return 1;
  return Math.max(0.12, (1 - (reach - shape.inner) / (shape.fade - shape.inner)) ** 1.5);
}

// hero.css's pill type: clamp(13px, 0.9vw, 16px), at a slot's size.
const typeOf = (width) => Math.min(16, Math.max(13, width * 0.009));
// A pill's width from its words, a little over the truth, so slots in a
// row never touch: half an em a character, and 3.35em of mark, gap and
// padding (hero.css).
const pillWidth = (text, type) => (text.length * 0.5 + 3.35) * type;

function notesOf(next, harnesses) {
  const first = ROUTINE_EVENTS[Math.floor(next() * ROUTINE_EVENTS.length)];
  const rest = ROUTINE_EVENTS.filter((text) => text !== first);
  const second = rest[Math.floor(next() * rest.length)];
  return [first, second].map((text) => ({ text, harness: harnesses[Math.floor(next() * harnesses.length)] }));
}

// Whether the shape keeps a slot centred at `point` that reaches down to
// `bottom`: the full field keeps all; the dome only what its mask shows,
// above the clear line, thinning as it goes out.
function keeps(shape, point, bottom, next) {
  if (shape.kind === "full") return true;
  const reach = domeReach(shape, point);
  return bottom <= shape.ry && reach < shape.fade && next() < density(shape, reach);
}

/** Every slot of the wall, row by row, in the field's px: its centre, its
 *  size, the most light it shows (`peak`) and its two notifications.
 *  Seeded, so a layout is the same on every build. */
export function wallSlots({ width, height, narrow, shape, seed = 316, harnesses = SUPPORTED_HARNESSES }) {
  const next = random(seed);
  const type = typeOf(width);
  const sizes = narrow ? SIZES.narrow : SIZES.wide;
  // Rows closer together than a pill is tall, so they overlap a little and
  // the wall reads as layers, as the lanes did.
  const pitch = type * 2 * sizes[1] * 0.8;
  const bottom = shape.kind === "full" ? height + pitch / 2 : shape.ry;
  const slots = [];
  for (let row = 0, y = pitch * 0.55; y - pitch / 2 < bottom; row += 1, y += pitch) {
    let x = -next() * 80;
    while (x < width) {
      const size = round(between(next, ...sizes), 2);
      const notes = notesOf(next, harnesses);
      const slotWidth = Math.max(...notes.map((note) => pillWidth(note.text, type * size)));
      const centre = [round(x + slotWidth / 2), round(y + between(next, -2, 2))];
      const slotHeight = 2 * type * size;
      if (x + slotWidth > 0 && keeps(shape, centre, centre[1] + slotHeight / 2, next)) {
        slots.push({ row, x: centre[0], y: centre[1], width: round(slotWidth), height: round(slotHeight), size, peak: round(between(next, 0.5, 1), 2), notes });
      }
      x += slotWidth + between(next, 6, 22) * (type / 14);
    }
  }
  return slots;
}

// A short slide from one of eight directions.
function slideFrom(next) {
  const angle = (Math.floor(next() * 8) * Math.PI) / 4;
  const distance = between(next, 8, 20);
  return [round(Math.cos(angle) * distance), round(Math.sin(angle) * distance)];
}

function notePlan(next) {
  return {
    in: round(between(next, 0.16, 0.28), 3),
    hold: round(between(next, 0.3, 0.8), 3),
    out: round(between(next, 0.32, 0.5), 3),
    gap: round(between(next, 0, 0.12), 3),
    from: slideFrom(next),
    drift: [round(between(next, -6, 6)), round(between(next, -10, -3))],
  };
}

/** A slot's pace: its two notifications, when each starts within the
 *  slot's `period`, and where in that period the slot is at time 0. */
export function planSlot(index, seed = 316) {
  const next = random((seed * 65_537) ^ Math.imul(index + 1, 2_654_435_761));
  const [first, second] = [notePlan(next), notePlan(next)];
  const secondStart = first.in + first.hold + first.gap;
  const period = Math.max(secondStart + second.in + second.hold + second.gap, first.in + first.hold + first.out + 0.02);
  const notes = [{ ...first, start: 0 }, { ...second, start: secondStart }];
  return { period, phase: -next() * period, notes };
}

const lengthOf = (note) => note.in + note.hold + note.out;

// The note's turns whose time on the wall meets [from, until), the last
// beginning before `exit`; one under way at the exit holds no longer.
function cyclesBetween(plan, index, from, until, exit) {
  const note = plan.notes[index];
  const first = Math.floor((from - lengthOf(note) - plan.phase - note.start) / plan.period);
  const cycles = [];
  for (let turn = first; ; turn += 1) {
    const start = plan.phase + turn * plan.period + note.start;
    if (start >= Math.min(until, exit)) break;
    if (start + lengthOf(note) <= from) continue;
    const hold = Math.min(note.hold, Math.max(0, exit - start - note.in));
    cycles.push({ ...note, start, hold });
  }
  return cycles;
}

/** A note's turns for its keyframes. Finite (`exit`): every turn from the
 *  start of the beat until the slot's exit, on the beat's clock. Endless:
 *  its one turn, on its own clock from 0; the animation loops it over the
 *  slot's period. */
export function slotCycles(plan, index, { exit }) {
  if (exit === Infinity) return [{ ...plan.notes[index], start: 0 }];
  return cyclesBetween(plan, index, 0, exit, exit);
}

/** What a note is doing at `time`: sliding "in", "shown", blurring "out",
 *  or null when it is off the wall. */
export function noteAt(plan, index, time, exit = Infinity) {
  const cycle = cyclesBetween(plan, index, time, time + 1e-9, exit).find((turn) => turn.start <= time);
  if (!cycle) return null;
  const into = time - cycle.start;
  if (into < cycle.in) return "in";
  if (into < cycle.in + cycle.hold) return "shown";
  return into < lengthOf(cycle) ? "out" : null;
}

// CSS's cubic-bezier timing, so a pose read between keyframes is where
// the browser has it.
function bezier(x1, y1, x2, y2) {
  const at = (a, b, t) => 3 * a * t * (1 - t) ** 2 + 3 * b * t ** 2 * (1 - t) + t ** 3;
  return (x) => {
    let low = 0;
    let high = 1;
    for (let step = 0; step < 30; step += 1) {
      const middle = (low + high) / 2;
      if (at(x1, x2, middle) < x) low = middle;
      else high = middle;
    }
    return at(y1, y2, (low + high) / 2);
  };
}

const EASES = {
  linear: (p) => p,
  ...Object.fromEntries(Object.entries(WALL_EASE).map(([name, css]) => [name, bezier(...css.match(/[\d.]+/g).map(Number))])),
};
const CSS_EASE = { linear: "linear", ...WALL_EASE };

const HIDDEN = Object.freeze({ opacity: 0, x: 0, y: 0, scale: 1, blur: 0 });

// The points a note's turn passes through, each with the ease to the next.
// Sharp: in from its side, shown, then gone (blurred, with a filter; or
// handed to its blurred copy half way). Blurred: rises as the sharp one
// goes, drifting on, then gone.
function turnPoints(cycle, { peak, blur, copy }) {
  const shownAt = cycle.start + cycle.in;
  const outAt = shownAt + cycle.hold;
  const [dx, dy] = cycle.drift;
  if (copy) {
    return [
      { t: outAt, pose: HIDDEN, ease: "linear" },
      { t: outAt + cycle.out * 0.4, pose: { ...HIDDEN, opacity: peak * 0.85, x: dx * 0.4, y: dy * 0.4, scale: 1.025 }, ease: "out" },
      { t: outAt + cycle.out, pose: { ...HIDDEN, x: dx, y: dy, scale: 1.06 }, ease: "linear" },
    ];
  }
  const share = blur === "filter" ? 1 : 0.5;
  return [
    { t: cycle.start, pose: { ...HIDDEN, x: cycle.from[0], y: cycle.from[1], scale: 0.94 }, ease: "in" },
    { t: shownAt, pose: { ...HIDDEN, opacity: peak }, ease: "linear" },
    { t: outAt, pose: { ...HIDDEN, opacity: peak }, ease: "out" },
    { t: outAt + cycle.out * share, pose: { ...HIDDEN, x: dx * share, y: dy * share, scale: 1 + 0.06 * share, blur: blur === "filter" ? BLUR_PX : 0 }, ease: "linear" },
  ];
}

const mix = (from, to, p) => Object.fromEntries(Object.keys(from).map((key) => [key, from[key] + (to[key] - from[key]) * p]));

// The pose the points give at `time`, as the browser would ease it.
function poseAt(points, time) {
  const after = points.findIndex((point) => point.t > time);
  if (after === -1) return points.at(-1).pose;
  if (after === 0) return points[0].pose;
  const before = points[after - 1];
  const p = (time - before.t) / (points[after].t - before.t);
  return mix(before.pose, points[after].pose, EASES[before.ease](p));
}

// The points within [0, span], with a point of its own at each end.
function clip(points, span) {
  const inside = points.filter((point) => point.t > 0 && point.t < span);
  const first = { t: 0, pose: poseAt(points, 0), ease: "linear" };
  const kept = points.find((point) => point.t === 0);
  if (kept) first.ease = kept.ease;
  else if (points.some((point) => point.t < 0)) first.ease = "linear";
  else if (points.length) first.pose = points[0].pose;
  return [first, ...inside, { t: span, pose: poseAt(points, span), ease: "linear" }];
}

function frameOf({ t, pose, ease }, span, withFilter) {
  const frame = {
    offset: round(t / span, 5),
    easing: CSS_EASE[ease],
    transform: `translate(${round(pose.x)}px, ${round(pose.y)}px) scale(${round(pose.scale, 3)})`,
    opacity: String(round(pose.opacity, 3)),
  };
  if (withFilter) frame.filter = `blur(${round(pose.blur)}px)`;
  return frame;
}

function framesOf(cycles, options, span) {
  const points = cycles.flatMap((cycle) => turnPoints(cycle, options));
  if (!points.length) return [frameOf({ t: 0, pose: HIDDEN, ease: "linear" }, span, false), frameOf({ t: span, pose: HIDDEN, ease: "linear" }, span, false)];
  return clip(points, span).map((point) => frameOf(point, span, options.blur === "filter" && !options.copy));
}

/** The keyframes of a note's turns over `span` seconds: `sharp`, and with
 *  the "copy" blur, `blurred` for its copy drawn blurred (else null). Each
 *  peaks at the slot's `peak` and ends hidden. */
export function noteFrames(cycles, { span, peak, blur }) {
  return {
    sharp: framesOf(cycles, { peak, blur, copy: false }, span),
    blurred: blur === "copy" ? framesOf(cycles, { peak, blur, copy: true }, span) : null,
  };
}

/** The three slots the requests pop up in, in ATTENTION's order: in the
 *  dome's solid middle (or across the top of the full wall), left to
 *  right, apart. */
export function requestSlots(slots, shape, { width, clear }) {
  const depth = shape.kind === "dome" ? shape.ry : clear;
  const spread = Math.min(width * 0.22, 260);
  const candidates = shape.kind === "dome" ? slots.filter((slot) => domeReach(shape, [slot.x, slot.y]) < shape.inner) : slots;
  const chosen = [];
  for (let index = 0; index < 3; index += 1) {
    const target = [width / 2 + spread * (index - 1), depth * 0.5];
    const free = candidates.filter((slot) => !chosen.includes(slot));
    chosen.push(free.reduce((best, slot) => (Math.hypot(slot.x - target[0], slot.y - target[1]) < Math.hypot(best.x - target[0], best.y - target[1]) ? slot : best)));
  }
  return chosen;
}
