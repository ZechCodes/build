// A field of stationary notifications. Each position has its own clock;
// short arrivals are interleaved across the field, never swept along a row.
import { ATTENTION, HARNESS_NAMES, ROUTINE_EVENTS, SUPPORTED_HARNESSES, random } from '../../hero/field.js';
export { ATTENTION, HARNESS_NAMES };

export const WALL_TIMING = Object.freeze({
  field: [0, 2.4], ripple: [2.4, 3.35], laptop: [2.8, 4.35],
  converge: [3.2, 4.21], message: [4.05, 4.6], settle: [4.6, 5.05],
  flight: .65, landings: [3.85, 4.03, 4.21],
});
export const REQUEST_APPEAR = [2.18, 2.36, 2.54];
const clamp = value => Math.min(1, Math.max(0, value));

export function requestPositions({ width, height }) {
  const narrow = width < 768;
  const points = narrow ? [[.46, .30], [.53, .46], [.47, .62]] : [[.29, .35], [.51, .50], [.73, .65]];
  const cardWidth = narrow ? 242 : 260;
  return points.map(([x, y]) => ({ x: Math.max(cardWidth / 2 + 16, Math.min(width - cardWidth / 2 - 16, x * width)), y: y * height, width: cardWidth, height: 66 }));
}

function clearance(slot, requests, { width, height }) {
  const request = requests.findIndex(point => Math.abs(point.x - slot.x) < (point.width + slot.width) / 2 + 12 && Math.abs(point.y - slot.y) < (point.height + slot.height) / 2 + 10);
  if (request !== -1) return REQUEST_APPEAR[request] - .12;
  const distance = Math.hypot((slot.x / width - .5) * 1.3, slot.y / height - .5);
  return 2.55 + Math.min(.65, distance * .9);
}

function turnsFor(slot, index, next, clearAt) {
  const enter = .32;
  const hold = 1.24 + next() * .36;
  const fade = .46;
  const period = enter + hold + fade + .10;
  // An irrational phase step scatters ages without row-sized waves.
  const phase = ((index * .61803398875 + .17) % 1) * period;
  const turns = [];
  for (let start = -phase, turn = 0; start < 2.4; start += period, turn += 1) {
    const outAt = Math.min(start + enter + hold, clearAt - fade);
    if (outAt <= start + enter) continue;
    const event = (index * 7 + turn * 13) % ROUTINE_EVENTS.length;
    turns.push({ start, enter, hold: outAt - start - enter, fade, from: [index % 3 === 0 ? -24 : 24, 8], text: ROUTINE_EVENTS[event], harness: SUPPORTED_HARNESSES[(index + turn) % 3] });
  }
  return turns;
}

export function createWall({ width, height }) {
  const next = random(336);
  const narrow = width < 768;
  const scale = Math.min(1.18, Math.max(1, width / 1920));
  const pitchX = narrow ? 184 : 224 * scale;
  const pitchY = narrow ? 68 : 78 * scale;
  const rows = Math.ceil(height / pitchY) + 1;
  const columns = Math.ceil(width / pitchX) + 1;
  const requests = requestPositions({ width, height });
  const slots = [];
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const slot = {
        x: (column + .12 + (row % 2) * .48) * pitchX,
        y: (row + .12) * pitchY + (next() - .5) * 5,
        width: (narrow ? 170 : 208) * scale,
        height: (narrow ? 54 : 62) * scale,
        depth: ['quiet', 'normal', 'normal', 'near'][(row * 3 + column) % 4],
      };
      slot.turns = turnsFor(slot, slots.length, next, clearance(slot, requests, { width, height }));
      slots.push(slot);
    }
  }
  return { slots, requests };
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
