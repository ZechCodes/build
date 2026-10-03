// A field of stationary notifications. Each position has its own clock;
// short arrivals are interleaved across the field, never swept along a row.
import { ATTENTION, HARNESS_NAMES, ROUTINE_EVENTS, SUPPORTED_HARNESSES, random } from '../../hero/field.js';
export { ATTENTION, HARNESS_NAMES };

export const WALL_TIMING = Object.freeze({
  field: [0, 4.8], ripple: [4.8, 5.75], laptop: [5.2, 6.75],
  converge: [5.6, 6.61], message: [6.45, 7], settle: [7, 7.45],
  flight: .65, landings: [6.25, 6.43, 6.61],
});
export const REQUEST_SELECT = [4.8, 4.94, 5.08];
const clamp = value => Math.min(1, Math.max(0, value));

function pickRequests(slots, { width, height }) {
  const points = width < 768 ? [[.46, .30], [.53, .46], [.47, .62]] : [[.29, .35], [.51, .50], [.73, .65]];
  const picked = [];
  return ATTENTION.map((entry, index) => {
    const [x, y] = points[index].map((value, axis) => value * [width, height][axis]);
    const candidates = slots.filter(slot => slot.x - slot.width / 2 >= 16 && slot.x + slot.width / 2 <= width - 16 && !picked.includes(slot));
    const slot = candidates.reduce((best, candidate) => Math.hypot(candidate.x - x, candidate.y - y) < Math.hypot(best.x - x, best.y - y) ? candidate : best);
    picked.push(slot);
    // Pick a turn already settled well before selection. Its earlier turns
    // remain ordinary churn, and this exact final card survives the fade.
    const turn = slot.turns.filter(turn => turn.start + turn.enter <= WALL_TIMING.field[1] - 1).at(-1);
    slot.turns = slot.turns.slice(0, slot.turns.indexOf(turn) + 1);
    Object.assign(turn, { attention: entry.id, text: entry.text, harness: entry.harness,
      hold: WALL_TIMING.landings[index] - WALL_TIMING.flight - turn.start - turn.enter });
    return { ...slot, slotIndex: slots.indexOf(slot), turn };
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
  return { slots, requests: pickRequests(slots, { width, height }) };
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
