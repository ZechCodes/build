// The painter for an agent bubble's face: a tiling from core/tilings.js, moved
// by the numbers from core/patternMotion.js, drawn inside the bubble's circle.
//
// What the rail hands over is one canvas, one pattern ordinal and one seed; what
// it gets back is a handle with four switches — working, ink, dimmed, destroyed.
// Everything else is settled here.
//
// Three things this module is careful about:
//
//  * ONE frame loop, not one per bubble. A rail can hold half a dozen agents,
//    and six requestAnimationFrame loops waking each other is six times the work
//    for one screen's worth of pixels. Renderers join a module-level set while
//    they are working; the loop starts when the set fills and cancels itself the
//    moment it empties, so an all-idle rail costs nothing at all.
//
//  * An idle bubble HOLDS ITS FRAME. Each renderer owns a clock that only counts
//    while that renderer is working, so stopping does not rewind the pattern to
//    the start — it stops where it stood. Ink and dim changes repaint that held
//    frame; they never advance it.
//
//  * The whole field moves together. Drift translates the lattice, rotation
//    tilts it, and the per-cell wave is the same wave sampled at each cell's own
//    place on it. Nothing is drawn per-cell-random: the motion has to read as one
//    surface breathing, not as a swarm.
//
// There are two faces, and the seed picks one. A 'grid' bubble draws its lattice
// and breathes it; a 'wavefill' bubble draws no lattice at all and lets a band of
// solid cells sweep across an empty circle. Everything above the per-cell paint —
// the tiling, the drift, the wrap, the one wave — is the same either way.
//
// jsdom has no 2D context — `getContext("2d")` is null there — so every paint
// path returns quietly rather than throwing. A bubble in a test still keeps its
// state; it just has nowhere to put pixels.

import { TILINGS, tilingForPattern } from "./tilings.js";
import {
  CELL_SCALE_BASE,
  cellFill,
  cellPhase,
  createClock,
  motionParams,
} from "./patternMotion.js";

const TAU = Math.PI * 2;

/** The longest frame delta the clock is allowed to see. A backgrounded tab
 *  hands back the whole time it was away on its first frame; without this the
 *  pattern jumps a minute forward in one step. */
const MAX_FRAME_MS = 64;

/** Retina is worth painting; beyond 3x the extra pixels are invisible on a
 *  36px bubble and the cost is not. */
const MAX_DEVICE_PIXEL_RATIO = 3;

/** One cell's edge, as a fraction of the bubble's width. Per tiling, because
 *  "edge" buys a very different amount of room in each: a hexagon is nearly two
 *  edges wide and an octagon's lattice steps 2.4 edges at a time, so a single
 *  fraction for all five would put five squares and two octagons in the same
 *  circle. These are chosen to land 3-5 cells across whichever tiling it is. */
const CELL_EDGE_FRACTION = {
  squares: 1 / 4.5,
  diamonds: 1 / 4,
  hexagons: 1 / 5,
  triangles: 1 / 4,
  octagons: 1 / 6,
};

/**
 * What one wrap of the drift is worth on each axis. `edges` is how far the
 * lattice has to travel, in cell edges, before it lies on top of itself again —
 * drift is wrapped to that, which is what makes the travel seamless: what is
 * drawn after the wrap differs from what would have been drawn without it by
 * exactly one lattice step, so nothing moves.
 *
 * `col` and `row` are the lattice INDICES that same step is worth, and they are
 * here because the wave reads a cell's absolute row and col. After a wrap the
 * cell covering a given point on the bubble is a different cell, this much
 * further along, so its phase would step with it — every cell at once, which is
 * a pop across the whole face. latticeDrift walks the phase back by exactly
 * this, so the wave stays where the viewer is looking.
 */
const LATTICE_WRAP = {
  squares: { x: { edges: 1, col: 1, row: 0 }, y: { edges: 1, col: 0, row: 1 } },
  // The rhombi are the square lattice turned 45 degrees, and so are their
  // indices: a step along one axis is a step along both diagonals at once.
  diamonds: {
    x: { edges: Math.SQRT2, col: 1, row: -1 },
    y: { edges: Math.SQRT2, col: 1, row: 1 },
  },
  // Odd rows are offset, so y repeats every two.
  hexagons: { x: { edges: Math.sqrt(3), col: 1, row: 0 }, y: { edges: 3, col: 0, row: 2 } },
  // col counts half-edges and the alternation flips each row, so a period is two
  // indices on either axis.
  triangles: { x: { edges: 1, col: 2, row: 0 }, y: { edges: Math.sqrt(3), col: 0, row: 2 } },
  octagons: {
    x: { edges: 1 + Math.SQRT2, col: 1, row: 0 },
    y: { edges: 1 + Math.SQRT2, col: 0, row: 1 },
  },
};

/** How much of a cell's alpha goes to its interior. The stroke carries the
 *  shape; the fill is there so the lattice reads as a surface rather than as
 *  wireframe. */
const FILL_ALPHA_SHARE = 0.4;

/** The 4.8.8 tiling's filler squares are grout between the octagons, not cells
 *  of their own — they are drawn far enough back to read that way. */
const FILLER_ALPHA_SHARE = 0.4;

/** What dimming costs the whole pattern. The unread count sits centred on top
 *  of a dimmed bubble and has to win. */
const DIMMED_ALPHA_SHARE = 0.3;

const STROKE_WIDTH_PX = 0.75;

/** The colour a bubble paints in when nobody has said. Callers set ink from the
 *  theme; this only has to be visible if one forgets. */
const DEFAULT_INK = "#ffffff";

// The renderers the shared loop is currently driving. A renderer is in here
// while it is working and out of it otherwise, so the set's size IS the answer
// to "does anything still need frames".
const animatingRenderers = new Set();
let framePending = false;
let frameHandle = null;
let lastFrameMs = 0;

/** How many renderers the shared loop is driving. Bookkeeping readout — the
 *  rail never needs it, tests and diagnostics do. */
export function animatingRendererCount() {
  return animatingRenderers.size;
}

function scheduleFrame() {
  if (framePending || animatingRenderers.size === 0) return;
  const request = globalThis.requestAnimationFrame;
  if (typeof request !== "function") return;
  framePending = true;
  frameHandle = request(runFrame);
}

function cancelFrame() {
  if (!framePending) return;
  const cancel = globalThis.cancelAnimationFrame;
  if (typeof cancel === "function") cancel(frameHandle);
  framePending = false;
  frameHandle = null;
  // The next renderer to start work begins a fresh timeline rather than
  // inheriting however long the rail sat idle.
  lastFrameMs = 0;
}

function runFrame(timestamp) {
  framePending = false;
  frameHandle = null;
  const nowMs = Number.isFinite(timestamp) ? timestamp : lastFrameMs;
  const deltaMs = lastFrameMs ? Math.min(nowMs - lastFrameMs, MAX_FRAME_MS) : 0;
  lastFrameMs = nowMs;
  // A copy: a renderer painting is free to stop or destroy itself, and that
  // would otherwise mutate the set mid-walk.
  for (const renderer of [...animatingRenderers]) renderer.advanceFrame(deltaMs);
  scheduleFrame();
}

function joinLoop(renderer) {
  animatingRenderers.add(renderer);
  scheduleFrame();
}

function leaveLoop(renderer) {
  if (!animatingRenderers.delete(renderer)) return;
  if (animatingRenderers.size === 0) cancelFrame();
}

/** Whether the reader has asked for less movement. matchMedia is missing in
 *  jsdom and in older embeddings, and an absent query is not a preference. */
function prefersReducedMotion() {
  const match = globalThis.matchMedia;
  if (typeof match !== "function") return false;
  const query = match("(prefers-reduced-motion: reduce)");
  return !!(query && query.matches);
}

/** Which of the five tilings an ordinal draws, by name — the name is what the
 *  density and period tables are keyed on. Found by identity rather than by
 *  index so the ordinal ordering stays tilings.js's business alone. */
function tilingNameFor(patternIndex) {
  const generator = tilingForPattern(patternIndex);
  return Object.keys(TILINGS).find((name) => TILINGS[name] === generator) || "squares";
}

function deviceRatio() {
  const ratio = Number(globalThis.devicePixelRatio) || 1;
  return Math.min(Math.max(ratio, 1), MAX_DEVICE_PIXEL_RATIO);
}

/** The bubble's CSS box. Zero while the element is still unlaid-out, which is
 *  a perfectly ordinary state to be painted in — the caller mounts the canvas
 *  and asks for a frame in the same tick. */
function cssBoxOf(canvas) {
  const measurable = typeof canvas.getBoundingClientRect === "function";
  const rect = measurable ? canvas.getBoundingClientRect() : null;
  const width = Math.max(0, (rect && rect.width) || canvas.clientWidth || 0);
  const height = Math.max(0, (rect && rect.height) || canvas.clientHeight || 0);
  return { width, height };
}

/** Travel split into whole lattice periods and what is left over: `offset` is
 *  what the painter translates by, small forever rather than growing with the
 *  session, and `wraps` is how many periods were shaved off to keep it that
 *  way — the count the wave has to be told about. */
function wrapTravel(distance, period) {
  if (!(period > 0)) return { offset: 0, wraps: 0 };
  const wraps = Math.floor(distance / period);
  return { offset: distance - wraps * period, wraps };
}

/**
 * Where the lattice sits at `seconds`, and what putting it there costs the wave.
 *
 *   offsetX, offsetY   the drift the painter translates the field by, wrapped
 *                      into one lattice period so the field lies on itself
 *   wavePhase          the phase that wrapping owes back, in radians
 *
 * Pure, and exported because this is the whole of the wrap: the geometry is
 * seamless by construction, and the phase is only seamless because of what is
 * subtracted here. See LATTICE_WRAP for why a cell's index moves at all.
 */
export function latticeDrift({ tilingName, params, cellSize, seconds }) {
  const wrap = LATTICE_WRAP[tilingName] || LATTICE_WRAP.squares;
  const travel = (seconds / params.driftSecondsPerCell) * cellSize;
  const across = wrapTravel(params.drift.x * travel, wrap.x.edges * cellSize);
  const down = wrapTravel(params.drift.y * travel, wrap.y.edges * cellSize);
  const wrappedCols = across.wraps * wrap.x.col + down.wraps * wrap.y.col;
  const wrappedRows = across.wraps * wrap.x.row + down.wraps * wrap.y.row;
  // Modulo a full turn: the correction is worth the same and stays small, however
  // long the bubble has been at work.
  const wavePhase = -(params.wave.kx * wrappedCols + params.wave.ky * wrappedRows) % TAU;
  return { offsetX: across.offset, offsetY: down.offset, wavePhase };
}

/**
 * The pattern engine for one bubble.
 *
 *   canvas        the element to paint into — sized here, from its CSS box
 *   patternIndex  the ordinal off the bubble (agentRailModel `agentPattern`)
 *   seed          the agent's id, so the same agent always moves the same way
 *
 * Returns the handle the rail holds: `setWorking` follows the agent, `setInk`
 * follows the theme, `setDimmed` follows the unread count sitting on top, and
 * `destroy` releases the bubble when its element goes away.
 */
export function createPatternRenderer({ canvas, patternIndex = 1, seed = "" } = {}) {
  const params = motionParams(seed);
  const clock = createClock();
  const tilingName = tilingNameFor(patternIndex);

  let context = null;
  let destroyed = false;
  let working = false;
  let dimmed = false;
  let ink = DEFAULT_INK;
  let field = null; // { cells, cellSize, size } — rebuilt when the box changes

  function contextOf() {
    if (context) return context;
    if (!canvas || typeof canvas.getContext !== "function") return null;
    context = canvas.getContext("2d") || null;
    return context;
  }

  /** Match the backing store to the CSS box, rebuilding the field whenever the
   *  bubble's size actually changed — resizing a canvas clears it, so this must
   *  not happen on frames where nothing moved. */
  function resize(box) {
    const size = Math.max(box.width, box.height);
    const ratio = deviceRatio();
    const pixelWidth = Math.round(box.width * ratio);
    const pixelHeight = Math.round(box.height * ratio);
    if (canvas.width !== pixelWidth) canvas.width = pixelWidth;
    if (canvas.height !== pixelHeight) canvas.height = pixelHeight;
    if (field && field.size === size) return;
    const cellSize = size * CELL_EDGE_FRACTION[tilingName];
    field = {
      size,
      cellSize,
      cells: TILINGS[tilingName](cellSize, { width: size, height: size }),
    };
  }

  function tracePolygon(ctx, cell, scale) {
    const [centerX, centerY] = cell.center;
    ctx.beginPath();
    for (let index = 0; index < cell.polygon.length; index += 1) {
      const [x, y] = cell.polygon[index];
      const px = centerX + (x - centerX) * scale;
      const py = centerY + (y - centerY) * scale;
      if (index === 0) ctx.moveTo(px, py);
      else ctx.lineTo(px, py);
    }
    ctx.closePath();
  }

  function paint() {
    if (destroyed || !canvas) return;
    const box = cssBoxOf(canvas);
    if (box.width <= 0 || box.height <= 0) return;
    const ctx = contextOf();
    if (!ctx) return;

    resize(box);
    const ratio = deviceRatio();
    const seconds = clock.now();
    const radius = Math.min(box.width, box.height) / 2;
    const centerX = box.width / 2;
    const centerY = box.height / 2;

    // From here on the context is in CSS pixels, whatever the backing store is.
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, box.width, box.height);
    ctx.save();
    ctx.beginPath();
    ctx.arc(centerX, centerY, radius, 0, TAU);
    ctx.clip();

    const { offsetX, offsetY, wavePhase } = latticeDrift({
      tilingName,
      params,
      cellSize: field.cellSize,
      seconds,
    });

    ctx.translate(centerX, centerY);
    ctx.rotate(params.rotationSpeed * seconds);
    ctx.translate(offsetX - field.size / 2, offsetY - field.size / 2);

    ctx.fillStyle = ink;
    ctx.strokeStyle = ink;
    ctx.lineWidth = STROKE_WIDTH_PX;

    const dimShare = dimmed ? DIMMED_ALPHA_SHARE : 1;
    const wavefill = params.style === "wavefill";
    // Rotation keeps a cell's distance from the field's centre, so culling on
    // that distance is exact whatever the tilt.
    const reach = radius + field.cellSize * 2;
    for (const cell of field.cells) {
      const dx = cell.center[0] + offsetX - field.size / 2;
      const dy = cell.center[1] + offsetY - field.size / 2;
      if (Math.hypot(dx, dy) > reach) continue;
      const kindShare =
        tilingName === "octagons" && cell.kind === "square" ? FILLER_ALPHA_SHARE : 1;
      if (wavefill) {
        // No stroke and no breathing: the lattice is invisible, and the only
        // thing that moves is which cells the crest has reached.
        tracePolygon(ctx, cell, CELL_SCALE_BASE);
        ctx.globalAlpha = cellFill(params, cell, seconds, wavePhase) * dimShare * kindShare;
        ctx.fill();
        continue;
      }
      const { scale, alpha } = cellPhase(params, cell, seconds, wavePhase);
      const cellAlpha = alpha * dimShare * kindShare;
      tracePolygon(ctx, cell, scale);
      ctx.globalAlpha = cellAlpha * FILL_ALPHA_SHARE;
      ctx.fill();
      ctx.globalAlpha = cellAlpha;
      ctx.stroke();
    }

    // restore() puts alpha, ink and the transform back where they were; the
    // next paint sets all three again from scratch anyway.
    ctx.restore();
  }

  const renderer = {
    advanceFrame(deltaMs) {
      if (destroyed) return;
      clock.advance(deltaMs);
      paint();
    },
  };

  paint();

  return {
    /** Follow the agent. Working runs the clock and joins the shared loop;
     *  idle stops both and leaves the last frame on the canvas. A reader who
     *  asked for reduced motion gets that frame and no loop at all. */
    setWorking(next) {
      if (destroyed) return;
      const wanted = !!next;
      if (wanted === working) return;
      working = wanted;
      if (!wanted) {
        clock.stop();
        leaveLoop(renderer);
        paint();
        return;
      }
      if (prefersReducedMotion()) {
        paint();
        return;
      }
      clock.start();
      joinLoop(renderer);
    },

    /** The colour the cells stroke and fill in. Repaints the held frame — the
     *  theme changing is not time passing. */
    setInk(cssColor) {
      if (destroyed) return;
      ink = cssColor || DEFAULT_INK;
      paint();
    },

    /** Drop the whole pattern back so something can sit on top of it. */
    setDimmed(next) {
      if (destroyed) return;
      dimmed = !!next;
      paint();
    },

    /** Release the bubble: out of the loop, clock stopped, every switch inert
     *  from here. Safe to call twice — a rail tearing down does not track which
     *  of its bubbles it has already let go. */
    destroy() {
      if (destroyed) return;
      destroyed = true;
      working = false;
      clock.stop();
      leaveLoop(renderer);
      field = null;
      context = null;
    },

    isWorking() {
      return working;
    },
  };
}
