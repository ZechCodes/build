// @vitest-environment jsdom
// The painter behind an agent bubble. There are no pixels to read here — jsdom
// hands back a null 2D context — so these tests hold it to the two things that
// are not about pixels: the state machine (working, ink, dim, destroy) and the
// one shared frame loop that every bubble on the rail rides.
//
// Where a frame's SHAPE matters, the canvas is a stand-in that records the calls
// the renderer makes. That is how a frozen bubble is shown to be frozen: repaint
// it and the ops come back identical, because no time passed.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
  createPatternRenderer,
  animatingRendererCount,
  latticeDrift,
} from "../src/core/agentCanvas.js";
import { cellFill, cellPhase, motionParams } from "../src/core/patternMotion.js";
import { TILINGS, pointInPolygon } from "../src/core/tilings.js";

/** A 2D context that remembers what it was told to do. Style assignments are
 *  recorded too — ink and alpha are the two things the renderer says rather
 *  than draws. */
function recordingContext() {
  const ops = [];
  const record =
    (op) =>
    (...args) =>
      ops.push({ op, args });
  const context = {
    ops,
    save: record("save"),
    restore: record("restore"),
    clearRect: record("clearRect"),
    setTransform: record("setTransform"),
    translate: record("translate"),
    rotate: record("rotate"),
    beginPath: record("beginPath"),
    closePath: record("closePath"),
    moveTo: record("moveTo"),
    lineTo: record("lineTo"),
    arc: record("arc"),
    clip: record("clip"),
    fill: record("fill"),
    stroke: record("stroke"),
  };
  for (const property of ["fillStyle", "strokeStyle", "globalAlpha", "lineWidth"]) {
    let value;
    Object.defineProperty(context, property, {
      get: () => value,
      set: (next) => {
        value = next;
        ops.push({ op: property, args: [next] });
      },
    });
  }
  return context;
}

function fakeCanvas({ width = 40, height = 40 } = {}) {
  const context = recordingContext();
  const canvas = {
    width: 0,
    height: 0,
    style: {},
    getContext: (kind) => (kind === "2d" ? context : null),
    getBoundingClientRect: () => ({ width, height, top: 0, left: 0, right: width, bottom: height }),
  };
  return { canvas, context };
}

/** A hand-cranked requestAnimationFrame: nothing runs until the test says so. */
function fakeFrames() {
  const pending = new Map();
  let nextHandle = 1;
  const request = vi.fn((callback) => {
    const handle = nextHandle++;
    pending.set(handle, callback);
    return handle;
  });
  const cancel = vi.fn((handle) => {
    pending.delete(handle);
  });
  const run = (timestamp) => {
    const callbacks = [...pending.values()];
    pending.clear();
    for (const callback of callbacks) callback(timestamp);
  };
  return { request, cancel, run, pending: () => pending.size };
}

const opsOf = (context, names) =>
  context.ops.filter(({ op }) => names.includes(op)).map(({ op, args }) => [op, ...args]);

/** The alphas the cells were drawn at. The reset the painter leaves behind on
 *  its way out of the clip is not one of them. */
function alphasOf(context) {
  const alphas = [];
  for (const { op, args } of context.ops) {
    if (op === "restore") break;
    if (op === "globalAlpha") alphas.push(args[0]);
  }
  return alphas;
}

const mean = (numbers) => numbers.reduce((total, value) => total + value, 0) / numbers.length;

/** The alpha each outline was drawn at by `paintOp`, filed under how many
 *  vertices it had — which is how an octagon is told from the filler square
 *  beside it. */
function alphasByVertexCount(context, paintOp) {
  const byVertices = new Map();
  let vertices = 0;
  let alpha = 0;
  for (const { op, args } of context.ops) {
    if (op === "beginPath") vertices = 0;
    else if (op === "moveTo" || op === "lineTo") vertices += 1;
    else if (op === "globalAlpha") alpha = args[0];
    else if (op === paintOp && vertices) {
      if (!byVertices.has(vertices)) byVertices.set(vertices, []);
      byVertices.get(vertices).push(alpha);
    }
  }
  return byVertices;
}

/** A seed wearing the face a test is about. Which of the two an agent gets is
 *  the seed's business alone, so a test that needs one goes looking. */
function seedWearing(style) {
  for (let index = 0; index < 500; index += 1) {
    const seed = `agent-${style}-${index}`;
    if (motionParams(seed).style === style) return seed;
  }
  throw new Error(`no seed drew the ${style} face`);
}

/** How many vertices each traced outline had — the fingerprint of the tiling
 *  underneath, since the clip path traces an arc and no vertices at all. */
function polygonSizes(context) {
  const sizes = new Set();
  let vertices = 0;
  for (const { op } of context.ops) {
    if (op === "beginPath") vertices = 0;
    else if (op === "moveTo" || op === "lineTo") vertices += 1;
    else if (op === "closePath" && vertices) sizes.add(vertices);
  }
  return sizes;
}

let frames;

beforeEach(() => {
  frames = fakeFrames();
  vi.stubGlobal("requestAnimationFrame", frames.request);
  vi.stubGlobal("cancelAnimationFrame", frames.cancel);
  vi.stubGlobal("devicePixelRatio", 2);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("createPatternRenderer", () => {
  it("starts idle and asks for no frames", () => {
    const { canvas } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-1" });
    expect(renderer.isWorking()).toBe(false);
    expect(frames.request).not.toHaveBeenCalled();
    renderer.destroy();
  });

  it("paints its first frame on creation, so an idle bubble still has a face", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-1" });
    expect(context.ops.some(({ op }) => op === "fill")).toBe(true);
    renderer.destroy();
  });

  it("sizes the canvas from its CSS box times the device pixel ratio", () => {
    const { canvas } = fakeCanvas({ width: 36, height: 36 });
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-1" });
    expect(canvas.width).toBe(72);
    expect(canvas.height).toBe(72);
    renderer.destroy();
  });

  it("clips every frame to the circle inscribed in the bubble", () => {
    const { canvas, context } = fakeCanvas({ width: 40, height: 40 });
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-1" });
    const [arcOp] = opsOf(context, ["arc"]);
    expect(arcOp.slice(0, 4)).toEqual(["arc", 20, 20, 20]);
    expect(context.ops.some(({ op }) => op === "clip")).toBe(true);
    renderer.destroy();
  });

  it("draws the tiling the pattern ordinal names", () => {
    const squares = fakeCanvas();
    const hexagons = fakeCanvas();
    const truncated = fakeCanvas();
    const one = createPatternRenderer({ canvas: squares.canvas, patternIndex: 1, seed: "a" });
    const two = createPatternRenderer({ canvas: hexagons.canvas, patternIndex: 2, seed: "a" });
    const five = createPatternRenderer({ canvas: truncated.canvas, patternIndex: 5, seed: "a" });
    expect(polygonSizes(squares.context)).toEqual(new Set([4]));
    expect(polygonSizes(hexagons.context)).toEqual(new Set([6]));
    expect(polygonSizes(truncated.context)).toEqual(new Set([8, 4]));
    one.destroy();
    two.destroy();
    five.destroy();
  });

  it("gives the same agent the same face and a different agent a different one", () => {
    const paintWith = (seed) => {
      const { canvas, context } = fakeCanvas();
      const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed });
      renderer.setWorking(true);
      frames.run(0);
      frames.run(1200);
      const frame = opsOf(context, ["translate", "rotate", "globalAlpha"]);
      renderer.destroy();
      return frame;
    };
    expect(paintWith("agent-7")).toEqual(paintWith("agent-7"));
    expect(paintWith("agent-7")).not.toEqual(paintWith("agent-8"));
  });
});

describe("the shared frame loop", () => {
  it("starts when a renderer begins working and keeps asking for the next frame", () => {
    const { canvas } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-1" });
    renderer.setWorking(true);
    expect(renderer.isWorking()).toBe(true);
    expect(frames.request).toHaveBeenCalledTimes(1);
    frames.run(16);
    expect(frames.request).toHaveBeenCalledTimes(2);
    renderer.destroy();
  });

  it("registers a renderer once however often it is told it is working", () => {
    const { canvas } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-1" });
    renderer.setWorking(true);
    renderer.setWorking(true);
    expect(frames.request).toHaveBeenCalledTimes(1);
    expect(animatingRendererCount()).toBe(1);
    renderer.setWorking(false);
    expect(animatingRendererCount()).toBe(0);
    renderer.destroy();
  });

  it("runs one loop for every working renderer, and cancels only when all are idle", () => {
    const first = createPatternRenderer({
      canvas: fakeCanvas().canvas,
      patternIndex: 1,
      seed: "a",
    });
    const second = createPatternRenderer({
      canvas: fakeCanvas().canvas,
      patternIndex: 2,
      seed: "b",
    });
    first.setWorking(true);
    second.setWorking(true);
    expect(frames.request).toHaveBeenCalledTimes(1);
    expect(animatingRendererCount()).toBe(2);

    first.setWorking(false);
    expect(frames.cancel).not.toHaveBeenCalled();
    second.setWorking(false);
    expect(frames.cancel).toHaveBeenCalledTimes(1);
    expect(frames.pending()).toBe(0);
    first.destroy();
    second.destroy();
  });

  it("unregisters a destroyed renderer and cancels the loop it was holding open", () => {
    const { canvas } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-1" });
    renderer.setWorking(true);
    renderer.destroy();
    expect(animatingRendererCount()).toBe(0);
    expect(frames.cancel).toHaveBeenCalledTimes(1);

    frames.request.mockClear();
    renderer.setWorking(true);
    expect(renderer.isWorking()).toBe(false);
    expect(frames.request).not.toHaveBeenCalled();
    renderer.destroy();
  });
});

describe("a working bubble against an idle one", () => {
  it("moves the whole field while it works", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 3, seed: "agent-move" });
    renderer.setWorking(true);
    frames.run(0);
    context.ops.length = 0;
    frames.run(500);
    const early = opsOf(context, ["translate", "rotate"]);
    context.ops.length = 0;
    frames.run(1500);
    expect(opsOf(context, ["translate", "rotate"])).not.toEqual(early);
    renderer.destroy();
  });

  it("freezes the frame it stopped on, and repaints it on ink and dim changes", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 2, seed: "agent-freeze" });
    renderer.setWorking(true);
    frames.run(0);
    frames.run(500);
    context.ops.length = 0;
    frames.run(1000);
    const lastLiveFrame = opsOf(context, ["translate", "rotate", "moveTo", "lineTo"]);

    context.ops.length = 0;
    renderer.setWorking(false);
    const frozenFrame = opsOf(context, ["translate", "rotate", "moveTo", "lineTo"]);
    expect(frozenFrame).toEqual(lastLiveFrame);

    // Frames keep running for other bubbles; this one must not move with them.
    context.ops.length = 0;
    renderer.setInk("#ff0000");
    expect(opsOf(context, ["translate", "rotate", "moveTo", "lineTo"])).toEqual(lastLiveFrame);
    expect(opsOf(context, ["fillStyle"])).toContainEqual(["fillStyle", "#ff0000"]);
    renderer.destroy();
  });

  it("keeps time still while idle, however many frames go by", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-still" });
    const busy = createPatternRenderer({ canvas: fakeCanvas().canvas, patternIndex: 1, seed: "b" });
    busy.setWorking(true);
    context.ops.length = 0;
    renderer.setInk("#123456");
    const atRest = opsOf(context, ["translate", "rotate"]);

    frames.run(0);
    frames.run(2000);
    frames.run(4000);
    context.ops.length = 0;
    renderer.setInk("#123456");
    expect(opsOf(context, ["translate", "rotate"])).toEqual(atRest);
    renderer.destroy();
    busy.destroy();
  });
});

describe("ink and dimming", () => {
  it("paints in the ink it was given", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-ink" });
    context.ops.length = 0;
    renderer.setInk("rgb(10, 20, 30)");
    expect(opsOf(context, ["fillStyle"])).toContainEqual(["fillStyle", "rgb(10, 20, 30)"]);
    expect(opsOf(context, ["strokeStyle"])).toContainEqual(["strokeStyle", "rgb(10, 20, 30)"]);
    renderer.destroy();
  });

  it("lowers every cell's alpha when dimmed, without moving the pattern", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: seedWearing("grid") });
    context.ops.length = 0;
    renderer.setInk("#fff");
    const bright = alphasOf(context);
    const geometry = opsOf(context, ["translate", "rotate", "moveTo", "lineTo"]);

    context.ops.length = 0;
    renderer.setDimmed(true);
    const dimmed = alphasOf(context);
    expect(dimmed).toHaveLength(bright.length);
    expect(dimmed.every((alpha, index) => alpha < bright[index])).toBe(true);
    expect(opsOf(context, ["translate", "rotate", "moveTo", "lineTo"])).toEqual(geometry);

    context.ops.length = 0;
    renderer.setDimmed(false);
    expect(alphasOf(context)).toEqual(bright);
    renderer.destroy();
  });

  it("paints the truncated tiling's filler squares fainter than its octagons", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({
      canvas,
      patternIndex: 5,
      seed: seedWearing("grid"),
    });
    context.ops.length = 0;
    renderer.setInk("#fff");

    // The wave puts every cell at its own point in the breath, so no single
    // filler is bound to be fainter than every octagon. Averaged over the field
    // the breath cancels and only the holding-back is left.
    const byVertices = alphasByVertexCount(context, "stroke");
    const octagons = byVertices.get(8) || [];
    const fillers = byVertices.get(4) || [];
    expect(octagons.length).toBeGreaterThan(3);
    expect(fillers.length).toBeGreaterThan(3);
    expect(mean(fillers)).toBeLessThan(mean(octagons) / 2);
    renderer.destroy();
  });
});

// Half the rail wears the other face: no lattice at all, just a band of solid
// cells sweeping over an empty circle. Same tiling, same one wave — the only
// difference is the shaping, so most cells sit at nothing and the crest fills in.

describe("the wavefill face", () => {
  const wavefillSeed = seedWearing("wavefill");
  const gridSeed = seedWearing("grid");

  it("never strokes: there is no lattice to see, only what the wave fills in", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: wavefillSeed });
    renderer.setWorking(true);
    frames.run(0);
    frames.run(900);
    expect(context.ops.some(({ op }) => op === "fill")).toBe(true);
    expect(context.ops.some(({ op }) => op === "stroke")).toBe(false);
    renderer.destroy();
  });

  it("leaves the grid face stroking exactly as it did", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: gridSeed });
    expect(context.ops.some(({ op }) => op === "stroke")).toBe(true);
    renderer.destroy();
  });

  it("holds most of the face at nothing while the crest goes to solid", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: wavefillSeed });
    renderer.setWorking(true);
    frames.run(0);

    const blank = [];
    let seenSolid = false;
    for (let step = 1; step <= 40; step += 1) {
      context.ops.length = 0;
      frames.run(step * 40);
      const alphas = alphasOf(context);
      expect(alphas.length).toBeGreaterThan(3);
      blank.push(alphas.filter((alpha) => alpha < 0.1).length / alphas.length);
      if (alphas.some((alpha) => alpha > 0.9)) seenSolid = true;
    }
    expect(mean(blank)).toBeGreaterThan(0.35);
    expect(seenSolid).toBe(true);
    renderer.destroy();
  });

  it("takes the dimming multiplier the same way the grid face does", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: wavefillSeed });
    context.ops.length = 0;
    renderer.setInk("#fff");
    const bright = alphasOf(context);

    context.ops.length = 0;
    renderer.setDimmed(true);
    const dimmed = alphasOf(context);
    expect(dimmed).toHaveLength(bright.length);
    const share = bright.map((alpha, index) => (alpha > 0 ? dimmed[index] / alpha : null));
    for (const ratio of share.filter((value) => value !== null)) {
      expect(ratio).toBeCloseTo(share.find((value) => value !== null), 12);
    }
    expect(share.some((value) => value !== null && value < 1)).toBe(true);
    renderer.destroy();
  });

  it("keeps the truncated tiling's filler squares on their fainter share", () => {
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 5, seed: wavefillSeed });
    renderer.setWorking(true);
    frames.run(0);

    const octagons = [];
    const fillers = [];
    for (let step = 1; step <= 20; step += 1) {
      context.ops.length = 0;
      frames.run(step * 60);
      const byVertices = alphasByVertexCount(context, "fill");
      octagons.push(...(byVertices.get(8) || []));
      fillers.push(...(byVertices.get(4) || []));
    }
    expect(octagons.length).toBeGreaterThan(20);
    expect(fillers.length).toBeGreaterThan(20);
    expect(mean(fillers)).toBeLessThan(mean(octagons) / 2);
    renderer.destroy();
  });

  it("does not jiggle the cells: the fill carries the motion on its own", () => {
    // A cell traced at rest scale lands on the same vertices every frame; one
    // that breathes lands somewhere new each time. Cells drift in and out of
    // the cull between frames, so this asks how much of the field held still,
    // not that all of it did.
    const heldStill = (seed) => {
      const { canvas, context } = fakeCanvas();
      const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed });
      renderer.setWorking(true);
      frames.run(0);
      context.ops.length = 0;
      frames.run(200);
      const early = new Set(opsOf(context, ["moveTo", "lineTo"]).map(String));
      context.ops.length = 0;
      frames.run(400);
      const later = opsOf(context, ["moveTo", "lineTo"]).map(String);
      renderer.destroy();
      return later.filter((vertex) => early.has(vertex)).length / later.length;
    };
    expect(heldStill(wavefillSeed)).toBeGreaterThan(0.9);
    expect(heldStill(gridSeed)).toBeLessThan(0.1);
  });
});

describe("prefers-reduced-motion", () => {
  it("draws the first frame and never asks for another", () => {
    vi.stubGlobal("matchMedia", () => ({ matches: true }));
    const { canvas, context } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-calm" });
    context.ops.length = 0;
    renderer.setWorking(true);
    expect(context.ops.some(({ op }) => op === "fill")).toBe(true);
    expect(frames.request).not.toHaveBeenCalled();
    expect(animatingRendererCount()).toBe(0);
    expect(renderer.isWorking()).toBe(true);
    renderer.destroy();
  });

  it("is not required to exist", () => {
    vi.stubGlobal("matchMedia", undefined);
    const { canvas } = fakeCanvas();
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-nomedia" });
    renderer.setWorking(true);
    expect(frames.request).toHaveBeenCalledTimes(1);
    renderer.destroy();
  });
});

describe("a canvas with no 2D context", () => {
  it("runs the whole state machine without throwing", () => {
    const canvas = document.createElement("canvas");
    canvas.getBoundingClientRect = () => ({ width: 40, height: 40, top: 0, left: 0 });
    expect(canvas.getContext("2d")).toBe(null);

    const renderer = createPatternRenderer({ canvas, patternIndex: 4, seed: "agent-headless" });
    expect(() => {
      renderer.setInk("#abc");
      renderer.setDimmed(true);
      renderer.setWorking(true);
      frames.run(0);
      frames.run(16);
      renderer.setWorking(false);
      renderer.destroy();
      renderer.destroy();
    }).not.toThrow();
    expect(animatingRendererCount()).toBe(0);
  });

  it("survives a bubble that has not been laid out yet", () => {
    const { canvas } = fakeCanvas({ width: 0, height: 0 });
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-unsized" });
    expect(() => {
      renderer.setWorking(true);
      frames.run(0);
      frames.run(16);
    }).not.toThrow();
    renderer.destroy();
  });
});

// The travel is wrapped to one lattice period so the geometry is seamless, and
// the wave is keyed on a cell's absolute row and col. Those two facts fight:
// after a wrap the cell covering a given point on the bubble is a DIFFERENT
// cell, one lattice step further along, and its phase would step with it —
// every cell at once, which is a pop across the whole face. These tests watch
// one point on the bubble through a wrap and hold the picture to being the
// same picture.

/** A field with room for the probe point to sit well inside the overscan
 *  however far the lattice has drifted under it. */
const FIELD_SIZE = 40;
const CELL_SIZE = 9;

/** The point on the bubble the tests watch, kept off the lattice lines of all
 *  five tilings so the cell covering it is never in doubt. */
const PROBE_POINT = [17.31, 12.77];

/** Between two samples the drift moves the field by a fraction of a pixel; a
 *  wrap moves it by a whole lattice period. Anything past this is a wrap. */
const WRAP_JUMP_PX = CELL_SIZE / 10;

/** Motion that travels along one axis only, so each axis's wrap can be watched
 *  on its own. The wave is the steepest the model draws — the biggest phase
 *  step per cell there is, and so the worst case for a phase that jumps a whole
 *  cell at a wrap. */
const driftingAlong = (axis) => ({
  drift: axis === "x" ? { x: 1, y: 0 } : { x: 0, y: 1 },
  driftSecondsPerCell: 1.5,
  rotationSpeed: 0,
  wave: { kx: 1.1 * Math.cos(0.6), ky: 1.1 * Math.sin(0.6) },
  waveFrequency: 2.2,
  scaleAmplitude: 0.12,
  alphaAmplitude: 0.35,
  phase: 0.4,
  style: "grid",
  fillSharpness: 5,
});

const driftAt = (tilingName, params, seconds) =>
  latticeDrift({ tilingName, params, cellSize: CELL_SIZE, seconds });

const wrapped = (one, other) =>
  Math.abs(one.offsetX - other.offsetX) > WRAP_JUMP_PX ||
  Math.abs(one.offsetY - other.offsetY) > WRAP_JUMP_PX;

/** The wrap inside a bracketing pair of samples, squeezed down to the last bit
 *  of double precision. Both returned seconds paint the SAME picture — the
 *  geometry either side of a wrap is identical — so anything that differs
 *  between them is something the viewer would see happen. */
function squeezeWrap(tilingName, params, lowSeconds, highSeconds) {
  let low = lowSeconds;
  let high = highSeconds;
  const atLow = driftAt(tilingName, params, low);
  for (let index = 0; index < 60; index += 1) {
    const middle = (low + high) / 2;
    if (middle <= low || middle >= high) break;
    if (wrapped(atLow, driftAt(tilingName, params, middle))) high = middle;
    else low = middle;
  }
  return { before: low, after: high };
}

/** Every wrap the drift makes in the first `toSeconds`, on either axis. */
function wrapsWithin(tilingName, params, toSeconds, step = 1e-3) {
  const wraps = [];
  let previousSeconds = 0;
  let previous = driftAt(tilingName, params, 0);
  for (let index = 1; index * step <= toSeconds; index += 1) {
    const seconds = index * step;
    const drift = driftAt(tilingName, params, seconds);
    if (wrapped(previous, drift)) {
      wraps.push(squeezeWrap(tilingName, params, previousSeconds, seconds));
    }
    previousSeconds = seconds;
    previous = drift;
  }
  return wraps;
}

/** The cell covering the probe point at `seconds`, and the alpha it is drawn
 *  at — what the viewer is looking at, whatever the cell's index happens to be. */
function underProbe(tilingName, params, cells, seconds) {
  const drift = driftAt(tilingName, params, seconds);
  const probe = [PROBE_POINT[0] - drift.offsetX, PROBE_POINT[1] - drift.offsetY];
  const cell = cells.find((candidate) => pointInPolygon(probe, candidate.polygon)) || null;
  return {
    cell,
    alpha: cellPhase(params, cell, seconds, drift.wavePhase).alpha,
    // The other face rides the same wave, so it owes the same debt at a wrap.
    fill: cellFill(params, cell, seconds, drift.wavePhase),
  };
}

const fieldOf = (tilingName) =>
  TILINGS[tilingName](CELL_SIZE, { width: FIELD_SIZE, height: FIELD_SIZE });

describe("the wave across a drift wrap", () => {
  for (const tilingName of Object.keys(TILINGS)) {
    for (const axis of ["x", "y"]) {
      it(`holds ${tilingName} still where the ${axis} drift wraps`, () => {
        const params = driftingAlong(axis);
        const cells = fieldOf(tilingName);
        const [wrap] = wrapsWithin(tilingName, params, 40);
        expect(wrap).toBeTruthy();

        const before = underProbe(tilingName, params, cells, wrap.before);
        const after = underProbe(tilingName, params, cells, wrap.after);
        // The wrap renumbers the cell under the probe. A test that never saw
        // that happen would be proving nothing.
        expect([after.cell.col, after.cell.row]).not.toEqual([before.cell.col, before.cell.row]);
        expect(Math.abs(after.alpha - before.alpha)).toBeLessThan(1e-6);
        expect(Math.abs(after.fill - before.fill)).toBeLessThan(1e-6);
      });
    }
  }

  it("holds every tiling still through a minute of seeded drift", () => {
    Object.keys(TILINGS).forEach((tilingName, index) => {
      const params = motionParams(`agent-wrap-${index}`);
      const cells = fieldOf(tilingName);
      const wraps = wrapsWithin(tilingName, params, 60);
      expect(wraps.length).toBeGreaterThan(0);
      for (const wrap of wraps) {
        const before = underProbe(tilingName, params, cells, wrap.before);
        const after = underProbe(tilingName, params, cells, wrap.after);
        expect(Math.abs(after.alpha - before.alpha)).toBeLessThan(1e-6);
        expect(Math.abs(after.fill - before.fill)).toBeLessThan(1e-6);
      }
    });
  });
});
