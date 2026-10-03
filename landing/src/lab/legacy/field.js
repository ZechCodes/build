// The hero's notification field, as data. Seeded, so every build draws the
// same field and nothing changes on a rerender; HeroField.astro renders it
// and entrance.js moves it. No DOM here.
//
// The opening beat is a flood: many lanes, overlapping in depth, some
// running against the others, each at its own speed and size, the routine
// ones jostling up and down. Each lane is two tracks meeting at its anchor
// (a fraction of the field's width): `before` runs left from the anchor,
// nearest first, and `after` runs right from it. The whole lane drifts by
// `drift` vw over DRIFT_SECONDS (negative runs left), so the track it comes
// from carries enough pills to stay full while it does. An attention
// request is the first pill after its lane's anchor, in a lane that runs
// right and holds still, so it starts exactly where it should be read.

import { ATTENTION as REQUESTS, HARNESS_NAMES, ROUTINE_EVENTS, SUPPORTED_HARNESSES, random } from "../../hero/notifications.js";
export { HARNESS_NAMES, ROUTINE_EVENTS, SUPPORTED_HARNESSES, random };

// The former hero's three held lanes survive only in this lab.
const REQUEST_LANES = Object.freeze({
  review: Object.freeze({ lane: 10, anchor: 0.5, narrowAnchor: 0.34 }),
  approval: Object.freeze({ lane: 14, anchor: 0.58, narrowAnchor: 0.2 }),
  question: Object.freeze({ lane: 18, anchor: 0.4, narrowAnchor: 0.4 }),
});
export const ATTENTION = Object.freeze(REQUESTS.map((request) => Object.freeze({ ...request, ...REQUEST_LANES[request.id] })));

// Fast faint lanes behind, slower sharper ones in front. `drift` is vw over
// DRIFT_SECONDS; `gap` is the px between pills on a 1440px window, and
// scales with the window like the pills do (hero.css); `size` scales a
// lane's pills, so depth reads in their size as well as their light.
export const DRIFT_SECONDS = 6;
// The CSS drift (hero.css, hero-drift) runs at that speed for the first
// 2.4 s, longer than the entrance ever lets it before taking over, then
// eases to rest by DRIFT_SECONDS, at this share of `drift`. A lane carries
// pills for that far, not for a drift that never ends.
export const DRIFT_REACH = 0.55;
export const LANE_TIERS = Object.freeze({
  far: Object.freeze({ drift: [150, 200], gap: [6, 48], size: [0.7, 0.84] }),
  mid: Object.freeze({ drift: [84, 118], gap: [8, 56], size: [0.88, 1] }),
  near: Object.freeze({ drift: [40, 58], gap: [12, 72], size: [1.08, 1.34] }),
});

// The lanes top to bottom: their tier, whether a phone keeps them, and
// which way they run (1 right, -1 left). Closer together than a pill is
// tall, so neighbouring lanes overlap and the field reads as layers.
const LANES = [
  ["far", false, -1], ["mid", true, 1], ["far", true, 1], ["near", true, 1],
  ["far", false, -1], ["mid", true, -1], ["near", false, 1], ["far", true, -1],
  ["mid", true, 1], ["far", true, 1], ["near", true, 1], ["far", false, -1],
  ["mid", true, -1], ["far", true, 1], ["near", true, 1], ["mid", false, 1],
  ["far", true, -1], ["mid", true, 1], ["near", true, 1], ["far", true, -1],
  ["near", false, 1], ["mid", true, -1], ["far", true, 1], ["near", true, 1],
];

// A pill's width in vw on a wide window, from hero.css: its words at about
// half an em a character, and its mark, gap and padding at 3.35em, at a
// type size of 0.78vw (the far tier's 15px at 1920) times its lane's size.
// A little under the truth, so a lane counted by it is never short.
export function pillWidthVw(text, size) {
  return (text.length * 0.46 + 3.35) * 0.78 * size;
}

// How much of a track's length a pill fills, gap included, in vw.
const spanVw = (pill, size) => pillWidthVw(pill.text, size) + Math.max(0, pill.gap) / 14.4;

const between = (next, [low, high]) => low + (high - low) * next();
const round = (value, places = 3) => Number(value.toFixed(places));

function checkHarnesses(harnesses) {
  if (!harnesses.length) throw new Error("The field needs at least one harness.");
  for (const id of harnesses) {
    if (!HARNESS_NAMES[id]) throw new Error(`The field has no mark for harness ${id}.`);
  }
}

// A phrase unlike the one before it in its track, used least so far.
function pickEvent(next, used, previous) {
  const candidates = ROUTINE_EVENTS.filter((text) => text !== previous);
  const fewest = Math.min(...candidates.map((text) => used.get(text) || 0));
  const least = candidates.filter((text) => (used.get(text) || 0) === fewest);
  const text = least[Math.floor(next() * least.length)];
  used.set(text, fewest + 1);
  return text;
}

function routinePill({ next, used, harnesses, tier }, previous, key) {
  const text = pickEvent(next, used, previous?.text);
  const harness = harnesses[Math.floor(next() * harnesses.length)];
  return { key, text, harness, gap: Math.round(between(next, LANE_TIERS[tier].gap)) };
}

// Pills until the track is `length` vw long, the first unlike `neighbour`
// (the pill across the anchor).
function track({ length, size, lead = null, neighbour = null, key, ...context }) {
  const pills = lead ? [lead] : [];
  let filled = pills.reduce((sum, pill) => sum + spanVw(pill, size), 0);
  while (filled < length) {
    const pill = routinePill(context, pills.at(-1) || neighbour, `${key}-${pills.length}`);
    pills.push(pill);
    filled += spanVw(pill, size);
  }
  return pills;
}

function attentionLead(attention, harnesses) {
  if (!attention) return null;
  const harness = harnesses.includes(attention.harness) ? attention.harness : harnesses[0];
  return { key: `attention-${attention.id}`, text: attention.text, harness, gap: 0, attention: attention.id };
}

// A request's lane holds still and at full size, so the request is read
// where it starts and lands where it was aimed.
function laneMotion(next, tier, attention) {
  const size = round(between(next, LANE_TIERS[tier].size), 2);
  const sway = Math.round(between(next, [3, 9]));
  const swaySeconds = round(between(next, [0.4, 1.2]), 2);
  const swayDelay = round(-next() * swaySeconds, 2);
  return attention ? { size: 1, sway: 0, swaySeconds: 0, swayDelay: 0 } : { size, sway, swaySeconds, swayDelay };
}

function lane(index, [tier, narrow, direction], context) {
  const { next, harnesses } = context;
  const attention = ATTENTION.find((entry) => entry.lane === index);
  const drift = round(direction * between(next, LANE_TIERS[tier].drift), 2);
  const anchor = attention ? attention.anchor : round(0.05 + next() * 0.9);
  const motion = laneMotion(next, tier, attention);
  const incoming = Math.abs(drift) * DRIFT_REACH;
  const [left, right] = [anchor * 100, (1 - anchor) * 100];
  const shared = { ...context, tier, size: motion.size };
  const before = track({ ...shared, key: `lane-${index}-before`, length: drift > 0 ? left + incoming : left });
  return {
    index,
    tier,
    narrow,
    drift,
    anchor,
    narrowAnchor: attention ? attention.narrowAnchor : anchor,
    ...motion,
    // A lane's own small vertical offset, in px, so the rows are staggered.
    nudge: Math.round((next() - 0.5) * 24),
    before,
    after: track({ ...shared, key: `lane-${index}-after`, length: drift > 0 ? right : right + incoming, lead: attentionLead(attention, harnesses), neighbour: before[0] }),
  };
}

export function createField({ seed = 310, harnesses = SUPPORTED_HARNESSES } = {}) {
  checkHarnesses(harnesses);
  const context = { next: random(seed), used: new Map(), harnesses };
  let narrowRow = 0;
  const lanes = LANES.map((spec, index) => {
    const built = lane(index, spec, context);
    return spec[1] ? { ...built, narrowRow: narrowRow++ } : built;
  });
  return { lanes, narrowLanes: narrowRow };
}
