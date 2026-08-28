// The animation state behind an agent bubble's canvas pattern: what the lattice
// is doing at time t, with nothing drawn and no DOM touched. The painter reads
// these numbers and puts pixels somewhere; every decision about HOW a pattern
// moves is made here, where it can be tested.
//
// Two properties drive the whole module:
//
//  * The same seed always moves the same way. The parameters are drawn from a
//    seeded PRNG, so a repaint replays the same drift, the same tilt, the same
//    ripple — nothing is stored. What the seed MEANS is the caller's choice:
//    core/agentRail.js salts the agent's id once per page load, so an agent is
//    itself all session and something new tomorrow.
//
//  * A paused bubble holds its frame. Time comes from createClock, which only
//    accumulates while it is running — so an idle agent's pattern stops where it
//    stood instead of snapping back to the start. (agentRail.js keeps the button
//    element alive across repaints for the same reason.)
//
// The lattice is addressed in CELLS, not pixels: cell (col, row) with col/row
// integers, origin wherever the painter puts it. Drift is quoted in seconds per
// cell so one loop of travel is exactly one cell — at which point the lattice
// has slid onto itself and the tiling is seamless, with no jump to hide.

/** Cell scale and alpha at rest, the value the wave swings around. The drawn
 *  alpha amplitude tops out at 0.35, which keeps alpha inside [0.25, 0.95]:
 *  visible at its dimmest, never flat white at its brightest. */
export const CELL_SCALE_BASE = 1;
export const CELL_ALPHA_BASE = 0.6;

const TAU = Math.PI * 2;

const DRIFT_SECONDS_PER_CELL = { min: 1.5, max: 3.5 };
const ROTATION_SPEED_LIMIT = 0.08; // rad/s, either direction — a slow tilt, not a spin
const WAVE_NUMBER = { min: 0.35, max: 1.1 }; // radians of phase per cell of travel
const WAVE_FREQUENCY = { min: 0.9, max: 2.2 }; // rad/s: a 3-to-7 second breath
const SCALE_AMPLITUDE = { min: 0.04, max: 0.12 };
const ALPHA_AMPLITUDE = { min: 0.15, max: 0.35 };

/** How hard the wavefill face pinches its wave into a crest. Below this the
 *  band is a haze over the whole circle rather than a few solid cells; above it
 *  the crest is gone before the eye finds it. */
const FILL_SHARPNESS = { min: 2.5, max: 5 };

/** The wavefill face wants a steeper wave than the grid face, and it is the
 *  crest shaping that demands it: a bubble holds four or five cells across, so
 *  a shallow wave puts every visible cell in the trough at once for stretches
 *  of the loop — and a trough, once shaped, is nothing at all. At this many
 *  radians per cell the face spans a full cycle or more, so a crest band is
 *  always somewhere on it. */
const WAVEFILL_WAVE_NUMBER = { min: 1.6, max: 2.4 };

const between = (random, range) => range.min + random() * (range.max - range.min);

const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

/**
 * Mulberry32: the small, fast, well-distributed 32-bit PRNG. Returns a
 * `() => number` drawing floats in [0, 1) — a fresh generator per call, so two
 * generators on the same seed replay the same sequence.
 */
export function mulberry32(seed) {
  let state = seed >>> 0;
  return function next() {
    state = (state + 0x6d2b79f5) >>> 0;
    let mixed = Math.imul(state ^ (state >>> 15), 1 | state);
    mixed = (mixed + Math.imul(mixed ^ (mixed >>> 7), 61 | mixed)) ^ mixed;
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * FNV-1a over the string's UTF-16 code units, as an unsigned 32-bit integer.
 * This is how an agent id becomes a seed: order-sensitive and spread out enough
 * that two agents named one character apart do not end up moving alike.
 */
export function hashString(s) {
  const text = String(s ?? "");
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index++) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/**
 * Draw one agent's motion. `seed` is a number, or an agent id that gets hashed
 * into one. The draw order is fixed: adding a parameter means appending it, or
 * every existing agent starts moving differently.
 *
 * Shape:
 *   drift: { x, y }         unit vector — which way the lattice travels
 *   driftSecondsPerCell     one cell per loop, so the travel tiles seamlessly
 *   rotationSpeed           rad/s, signed, and freely near zero (some agents
 *                           simply do not tilt — that is a face too)
 *   wave: { kx, ky }        radians of phase added per column / per row, and
 *                           steeper on a wavefill face than on a grid one
 *   waveFrequency           rad/s the same phase advances in time
 *   scaleAmplitude          how far a cell breathes, as a fraction of its size
 *   alphaAmplitude          how far it brightens and dims
 *   phase                   where in the loop this agent starts
 *   style                   'grid' — a drawn lattice breathing — or 'wavefill',
 *                           a blank face with solid cells sweeping over it
 *   fillSharpness           how tight the wavefill crest is (see cellFill)
 */
export function motionParams(seed) {
  const random = mulberry32(typeof seed === "number" ? seed : hashString(seed));

  const driftAngle = random() * TAU;
  const driftSecondsPerCell = between(random, DRIFT_SECONDS_PER_CELL);
  const rotationSpeed = (random() * 2 - 1) * ROTATION_SPEED_LIMIT;
  // The wave gets an angle and a magnitude rather than two independent
  // components: drawing kx and ky separately can land both near zero, and a
  // lattice with no spatial phase is a lattice pulsing in lockstep — the one
  // motion this is meant to avoid.
  const waveAngle = random() * TAU;
  const waveNumber = between(random, WAVE_NUMBER);
  const waveFrequency = between(random, WAVE_FREQUENCY);
  const scaleAmplitude = between(random, SCALE_AMPLITUDE);
  const alphaAmplitude = between(random, ALPHA_AMPLITUDE);
  const phase = random() * TAU;
  // Appended, and the two faces split evenly: an agent is as likely to wear a
  // lattice as a band of solid cells, and which it is stays its own for as long
  // as the seed does.
  const style = random() < 0.5 ? "grid" : "wavefill";
  const fillSharpness = between(random, FILL_SHARPNESS);
  const wavefillWaveNumber = between(random, WAVEFILL_WAVE_NUMBER);

  // The wavefill face keeps the direction it drew and trades the magnitude. It
  // is done here, at the source, because everything downstream reads
  // params.wave — including agentCanvas's wrap compensation, which has to be
  // walking back the same wave the cells are riding.
  const spatialWaveNumber = style === "wavefill" ? wavefillWaveNumber : waveNumber;

  return {
    drift: { x: Math.cos(driftAngle), y: Math.sin(driftAngle) },
    driftSecondsPerCell,
    rotationSpeed,
    wave: {
      kx: spatialWaveNumber * Math.cos(waveAngle),
      ky: spatialWaveNumber * Math.sin(waveAngle),
    },
    waveFrequency,
    scaleAmplitude,
    alphaAmplitude,
    phase,
    style,
    fillSharpness,
  };
}

/**
 * A clock that only counts while it is running. `advance(dtMs)` takes the frame
 * delta the painter already has; `now()` reads back the accumulated running time
 * in SECONDS, which is what cellPhase wants.
 *
 * A fresh clock is stopped: a bubble is idle until its agent picks up work, and
 * an idle bubble is a still frame. Deltas that are negative or not finite are
 * dropped — a suspended tab handing back nonsense must not throw the pattern
 * forward or backward.
 */
export function createClock() {
  let elapsedSeconds = 0;
  let running = false;
  return {
    advance(dtMs) {
      if (!running) return;
      const delta = Number(dtMs);
      if (!Number.isFinite(delta) || delta <= 0) return;
      elapsedSeconds += delta / 1000;
    },
    start() {
      running = true;
    },
    stop() {
      running = false;
    },
    now() {
      return elapsedSeconds;
    },
  };
}

/**
 * One cell's scale and alpha at time `tSeconds` — the whole of the concerted
 * motion. Every cell rides ONE wave, sampled at its own place on the lattice:
 *
 *   angle = waveFrequency * t + kx * col + ky * row + phase
 *
 * so a neighbour is not doing its own thing, it is doing this cell's thing a
 * fixed moment later. That is what reads as a ripple crossing the pattern rather
 * than as every cell twitching on its own.
 *
 * `phaseCorrection` is the painter's to add: whatever it has done to the lattice
 * that the cell indices know nothing about. There is one such thing — wrapping
 * the drift renumbers the cell under any given point on the screen — and
 * agentCanvas.js `latticeDrift` is where the number comes from.
 *
 * Alpha is clamped to [0, 1] for painters that hand it straight to a context;
 * the drawn amplitudes never reach the clamp, so it changes nothing in practice.
 */
export function cellPhase(params, cell, tSeconds, phaseCorrection = 0) {
  const swing = Math.sin(waveAngleAt(params, cell, tSeconds, phaseCorrection));
  return {
    scale: CELL_SCALE_BASE + params.scaleAmplitude * swing,
    alpha: clamp(CELL_ALPHA_BASE + params.alphaAmplitude * swing, 0, 1),
  };
}

/**
 * One cell's fill alpha for the wavefill face, on the SAME wave cellPhase reads
 * — same angle, same `phaseCorrection` from the painter, so the two faces are
 * one motion shown two ways.
 *
 * What differs is the shaping. The swing is mapped into [0, 1] and raised to
 * `fillSharpness`, which flattens everything away from the peak towards nothing
 * and leaves only a narrow crest near 1. The face is blank; what crosses it is
 * a band of cells that happen to be at the crest right now.
 *
 * The cell's scale is not this function's business: a wavefill cell is drawn at
 * rest size always. The band travelling is the motion, and a cell resizing under
 * it would only read as jitter.
 */
export function cellFill(params, cell, tSeconds, phaseCorrection = 0) {
  const swing = Math.sin(waveAngleAt(params, cell, tSeconds, phaseCorrection));
  return clamp(((swing + 1) / 2) ** params.fillSharpness, 0, 1);
}

/** The one wave, sampled at this cell at this moment. Both faces read it. */
function waveAngleAt(params, cell, tSeconds, phaseCorrection) {
  return (
    params.waveFrequency * tSeconds +
    params.wave.kx * cell.col +
    params.wave.ky * cell.row +
    params.phase +
    phaseCorrection
  );
}
