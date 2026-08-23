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
import { createPatternRenderer, animatingRendererCount } from "../src/core/agentCanvas.js";

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

/** The alpha each outline was STROKED at, filed under how many vertices it had
 *  — which is how an octagon is told from the filler square beside it. */
function strokeAlphasByVertexCount(context) {
  const byVertices = new Map();
  let vertices = 0;
  let alpha = 0;
  for (const { op, args } of context.ops) {
    if (op === "beginPath") vertices = 0;
    else if (op === "moveTo" || op === "lineTo") vertices += 1;
    else if (op === "globalAlpha") alpha = args[0];
    else if (op === "stroke" && vertices) {
      if (!byVertices.has(vertices)) byVertices.set(vertices, []);
      byVertices.get(vertices).push(alpha);
    }
  }
  return byVertices;
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
    const renderer = createPatternRenderer({ canvas, patternIndex: 1, seed: "agent-dim" });
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
    const renderer = createPatternRenderer({ canvas, patternIndex: 5, seed: "agent-fill" });
    context.ops.length = 0;
    renderer.setInk("#fff");

    // The wave puts every cell at its own point in the breath, so no single
    // filler is bound to be fainter than every octagon. Averaged over the field
    // the breath cancels and only the holding-back is left.
    const byVertices = strokeAlphasByVertexCount(context);
    const octagons = byVertices.get(8) || [];
    const fillers = byVertices.get(4) || [];
    expect(octagons.length).toBeGreaterThan(3);
    expect(fillers.length).toBeGreaterThan(3);
    expect(mean(fillers)).toBeLessThan(mean(octagons) / 2);
    renderer.destroy();
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
