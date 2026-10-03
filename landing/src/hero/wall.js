// A field of stationary notifications. Each position has its own clock;
// short arrivals are interleaved across the field, never swept along a row.
import { ATTENTION, HARNESS_NAMES, ROUTINE_EVENTS, SUPPORTED_HARNESSES, random } from './notifications.js';
export { ATTENTION, HARNESS_NAMES };

export const WALL_TIMING = Object.freeze({
  field: [0, 3], ripple: [3, 3.95], laptop: [3.4, 5.725],
  converge: [4.575, 5.585], message: [5.425, 5.975], settle: [5.975, 6.425],
  flight: .65, landings: [5.225, 5.405, 5.585],
});
export const REQUEST_SELECT = [3, 3.14, 3.28];
const clamp = value => Math.min(1, Math.max(0, value));

const REQUEST_POINTS = Object.freeze({
  narrow: [[.46, .30], [.53, .46], [.47, .62]],
  wide: [[.29, .35], [.51, .50], [.73, .65]],
});

function targetFor(index, { width, height }) {
  const [x, y] = REQUEST_POINTS[width < 768 ? 'narrow' : 'wide'][index];
  return [x * width, y * height];
}

function nearestSlot(slots, picked, [x, y], width) {
  const unused = slots.filter(slot => !picked.has(slot));
  const inside = unused.filter(slot => slot.x - slot.width / 2 >= 16 && slot.x + slot.width / 2 <= width - 16);
  const candidates = inside.length ? inside : unused;
  if (!candidates.length) throw new Error('The wall needs a distinct slot for each request.');
  let nearest = candidates[0];
  let distance = Math.hypot(nearest.x - x, nearest.y - y);
  for (const candidate of candidates.slice(1)) {
    const next = Math.hypot(candidate.x - x, candidate.y - y);
    if (next < distance) {
      nearest = candidate;
      distance = next;
    }
  }
  return nearest;
}

function requestTurn(slot, selection) {
  const cutoff = WALL_TIMING.field[1] - 1;
  // A settled card is already part of the ordinary wall. If none was ready
  // by the preferred cutoff, use the latest one settled by selection.
  const settled = time => slot.turns.findLast(turn => turn.start + turn.enter <= time);
  const turn = settled(cutoff) ?? settled(selection);
  if (turn) return turn;
  const fallback = { start: cutoff - .32, enter: .32, hold: .72, fade: .28,
    from: [56, 0], text: ROUTINE_EVENTS[0], harness: SUPPORTED_HARNESSES[0] };
  // No card has arrived in time. Replace future turns with one already
  // settled; otherwise its extended hold could be negative at takeoff.
  slot.turns = [fallback];
  return fallback;
}

function selectRequest(slot, slotIndex, entry, index) {
  const turn = requestTurn(slot, REQUEST_SELECT[index]);
  slot.turns = slot.turns.slice(0, slot.turns.indexOf(turn) + 1);
  Object.assign(turn, { attention: entry.id, text: entry.text, harness: entry.harness,
    hold: WALL_TIMING.landings[index] - WALL_TIMING.flight - turn.start - turn.enter });
  return { ...slot, slotIndex, turn };
}

export function selectRequests(slots, size) {
  const picked = new Set();
  return ATTENTION.map((entry, index) => {
    const slot = nearestSlot(slots, picked, targetFor(index, size), size.width);
    picked.add(slot);
    return selectRequest(slot, slots.indexOf(slot), entry, index);
  });
}

function clearance(slot, { width, height }) {
  const distance = Math.hypot((slot.x / width - .5) * 1.3, slot.y / height - .5);
  return WALL_TIMING.field[1] + .15 + Math.min(.65, distance * .9);
}

function turnsFor(index, next, clearAt) {
  const enter = .32;
  const hold = .72 + next() * .16;
  const fade = .28;
  const period = enter + hold + fade + .025;
  // An irrational phase step scatters ages without row-sized waves.
  const phase = ((index * .61803398875 + .17) % 1) * period;
  const turns = [];
  for (let start = -phase, turn = 0; start < WALL_TIMING.field[1]; start += period, turn += 1) {
    const outAt = Math.min(start + enter + hold, clearAt - fade);
    if (outAt <= start + enter) continue;
    const event = (index * 7 + turn * 13) % ROUTINE_EVENTS.length;
    turns.push({ start, enter, hold: outAt - start - enter, fade, from: [56, 0], text: ROUTINE_EVENTS[event], harness: SUPPORTED_HARNESSES[(index + turn) % 3] });
  }
  return turns;
}

export function createWall({ width, height }) {
  const next = random(336);
  const narrow = width < 768;
  const scale = Math.min(1.18, Math.max(1, width / 1920));
  const pitchX = narrow ? 176 : 224 * scale;
  const pitchY = narrow ? 64 : 78 * scale;
  const rows = Math.ceil(height / pitchY) + 1;
  const columns = Math.ceil(width / pitchX) + 1;
  const slots = [];
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const slot = {
        x: (column + .12 + (row % 2) * .48) * pitchX,
        y: (row + .12) * pitchY + (next() - .5) * 5,
        width: (narrow ? 164 : 208) * scale,
        height: (narrow ? 54 : 62) * scale,
        depth: ['quiet', 'normal', 'normal', 'near'][(row * 3 + column) % 4],
      };
      slot.turns = turnsFor(slots.length, next, clearance(slot, { width, height }));
      slots.push(slot);
    }
  }
  return { slots, requests: selectRequests(slots, { width, height }) };
}

// Used by the browser keyframes and the plan tests. During the hold both
// coordinates are exactly zero; notifications never cruise through the page.
export function notePose(turn, time) {
  const age = time - turn.start;
  if (age < 0 || age >= turn.enter + turn.hold + turn.fade) return { x: 0, y: 0, opacity: 0 };
  if (age < turn.enter) {
    const p = 1 - (1 - clamp(age / turn.enter)) ** 3;
    return { x: turn.from[0] * (1 - p), y: turn.from[1] * (1 - p), opacity: p };
  }
  const gone = clamp((age - turn.enter - turn.hold) / turn.fade);
  return { x: 0, y: gone ? -3 * gone : 0, opacity: 1 - gone };
}
