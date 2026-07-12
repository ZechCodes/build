import { describe, it, expect } from "vitest";
import { createTouchScroll, createWheelQuantizer } from "../src/terminal/touchScroll.js";

const touch = (id, y) => ({ identifier: id, clientY: y, clientX: 0 });
const ev = (touches, t, changed = touches) => ({
  touches, changedTouches: changed, timeStamp: t, cancelable: true,
  prevented: false, preventDefault() { this.prevented = true; },
  stopped: false, stopPropagation() { this.stopped = true; },
});

// Harness: collects dispatched wheel deltas and fakes the frame scheduler so
// tests drive momentum with chosen timestamps.
function setup() {
  const dispatched = [];
  const frames = new Map();
  const cancelled = [];
  let nextFrameId = 1;
  const gesture = createTouchScroll({
    dispatchWheel: (deltaY) => dispatched.push(deltaY),
    requestFrame: (cb) => { const id = nextFrameId++; frames.set(id, cb); return id; },
    cancelFrame: (id) => { cancelled.push(id); frames.delete(id); },
  });
  const fireFrame = (t) => {
    const next = frames.entries().next().value;
    if (!next) throw new Error("no pending frame to fire");
    const [id, cb] = next;
    frames.delete(id);
    cb(t);
  };
  return { gesture, dispatched, frames, cancelled, fireFrame };
}

const sum = (deltas) => deltas.reduce((a, b) => a + b, 0);

// Drive a fast drag (well past slop, high velocity) and lift; leaves momentum pending.
function flick(gesture, { startT = 0 } = {}) {
  gesture.onTouchStart(ev([touch(1, 300)], startT));
  gesture.onTouchMove(ev([touch(1, 280)], startT + 16));
  gesture.onTouchMove(ev([touch(1, 260)], startT + 32));
  gesture.onTouchMove(ev([touch(1, 240)], startT + 48));
  gesture.onTouchEnd(ev([], startT + 56, [touch(1, 240)]));
}

describe("createTouchScroll", () => {
  it("dispatches positive deltas for an upward drag and negative for downward", () => {
    const { gesture, dispatched } = setup();
    gesture.onTouchStart(ev([touch(1, 300)], 0));
    gesture.onTouchMove(ev([touch(1, 260)], 20));
    gesture.onTouchMove(ev([touch(1, 230)], 40));
    gesture.onTouchMove(ev([touch(1, 200)], 60));
    expect(sum(dispatched)).toBeCloseTo(100);
    expect(dispatched.every((d) => d > 0)).toBe(true);

    const down = setup();
    down.gesture.onTouchStart(ev([touch(1, 200)], 0));
    down.gesture.onTouchMove(ev([touch(1, 250)], 20));
    down.gesture.onTouchMove(ev([touch(1, 300)], 40));
    expect(sum(down.dispatched)).toBeCloseTo(-100);
    expect(down.dispatched.every((d) => d < 0)).toBe(true);
  });

  it("keeps a tap a tap: sub-slop movement emits nothing and prevents nothing", () => {
    const { gesture, dispatched, frames } = setup();
    const start = ev([touch(1, 300)], 0);
    const move = ev([touch(1, 296)], 30);
    const end = ev([], 60, [touch(1, 296)]);
    gesture.onTouchStart(start);
    gesture.onTouchMove(move);
    gesture.onTouchEnd(end);
    expect(dispatched).toEqual([]);
    expect(frames.size).toBe(0);
    expect(start.prevented).toBe(false);
    expect(move.prevented).toBe(false);
    expect(end.prevented).toBe(false);
  });

  it("banks slop distance and emits it once the threshold is crossed", () => {
    const { gesture, dispatched } = setup();
    gesture.onTouchStart(ev([touch(1, 300)], 0));
    const first = ev([touch(1, 295)], 20); // 5px — inside slop
    gesture.onTouchMove(first);
    expect(dispatched).toEqual([]);
    expect(first.prevented).toBe(false);
    const second = ev([touch(1, 285)], 40); // +10px — past slop
    gesture.onTouchMove(second);
    expect(second.prevented).toBe(true);
    expect(sum(dispatched)).toBeCloseTo(15);
  });

  it("ends the gesture on a second finger, without momentum, until all lift", () => {
    const { gesture, dispatched, frames } = setup();
    gesture.onTouchStart(ev([touch(1, 300)], 0));
    gesture.onTouchMove(ev([touch(1, 280)], 16));
    expect(sum(dispatched)).toBeCloseTo(20);
    dispatched.length = 0;

    gesture.onTouchStart(ev([touch(1, 280), touch(2, 500)], 24, [touch(2, 500)]));
    gesture.onTouchMove(ev([touch(1, 200), touch(2, 500)], 40, [touch(1, 200)]));
    expect(dispatched).toEqual([]);

    gesture.onTouchEnd(ev([touch(2, 500)], 60, [touch(1, 200)]));
    gesture.onTouchMove(ev([touch(2, 480)], 70)); // still suppressed
    gesture.onTouchEnd(ev([], 80, [touch(2, 480)]));
    expect(dispatched).toEqual([]);
    expect(frames.size).toBe(0);

    // All fingers lifted — a fresh gesture scrolls again.
    gesture.onTouchStart(ev([touch(3, 300)], 100));
    gesture.onTouchMove(ev([touch(3, 260)], 120));
    expect(sum(dispatched)).toBeCloseTo(40);
  });

  it("continues scrolling with decaying momentum after a flick", () => {
    const { gesture, dispatched, frames, fireFrame } = setup();
    flick(gesture);
    const dragged = dispatched.length;
    expect(frames.size).toBe(1);

    let t = 1000;
    let guard = 0;
    while (frames.size > 0) {
      fireFrame(t);
      t += 16;
      if (++guard > 10000) throw new Error("momentum never stopped");
    }
    const momentum = dispatched.slice(dragged);
    expect(momentum.length).toBeGreaterThan(1);
    expect(momentum.every((d) => d > 0)).toBe(true);
    expect(momentum.at(-1)).toBeLessThan(momentum[0]);
  });

  it("skips momentum when the finger pauses before lifting", () => {
    const { gesture, frames } = setup();
    gesture.onTouchStart(ev([touch(1, 300)], 0));
    gesture.onTouchMove(ev([touch(1, 250)], 50));
    gesture.onTouchEnd(ev([], 200, [touch(1, 250)])); // 150ms stationary
    expect(frames.size).toBe(0);
  });

  it("cancels momentum when a new touch starts", () => {
    const { gesture, dispatched, frames, cancelled, fireFrame } = setup();
    flick(gesture);
    fireFrame(1000);
    fireFrame(1016);
    const before = dispatched.length;

    gesture.onTouchStart(ev([touch(4, 400)], 1020));
    expect(cancelled.length).toBeGreaterThan(0);
    expect(frames.size).toBe(0);
    expect(dispatched.length).toBe(before);
  });

  it("dispose() cancels a pending momentum frame", () => {
    const { gesture, frames, cancelled } = setup();
    flick(gesture);
    expect(frames.size).toBe(1);
    gesture.dispose();
    expect(cancelled.length).toBe(1);
    expect(frames.size).toBe(0);
  });

  it("stops touchend propagation after a drag, but not after a tap", () => {
    // ghostty-web's canvas touchend handler focuses the hidden textarea (mobile
    // keyboard); it must fire for taps and stay silenced after a scroll drag.
    const drag = setup();
    drag.gesture.onTouchStart(ev([touch(1, 300)], 0));
    drag.gesture.onTouchMove(ev([touch(1, 260)], 20));
    const dragEnd = ev([], 40, [touch(1, 260)]);
    drag.gesture.onTouchEnd(dragEnd);
    expect(dragEnd.stopped).toBe(true);

    const tap = setup();
    tap.gesture.onTouchStart(ev([touch(1, 300)], 0));
    const tapEnd = ev([], 40, [touch(1, 300)]);
    tap.gesture.onTouchEnd(tapEnd);
    expect(tapEnd.stopped).toBe(false);
  });

  it("stops touchend propagation when a multi-touch gesture ends", () => {
    const { gesture } = setup();
    gesture.onTouchStart(ev([touch(1, 300)], 0));
    gesture.onTouchStart(ev([touch(1, 300), touch(2, 500)], 10, [touch(2, 500)]));
    const end = ev([], 30, [touch(1, 300), touch(2, 500)]);
    gesture.onTouchEnd(end);
    expect(end.stopped).toBe(true);
  });

  it("skips preventDefault on a non-cancelable touchend", () => {
    const { gesture } = setup();
    gesture.onTouchStart(ev([touch(1, 300)], 0));
    gesture.onTouchMove(ev([touch(1, 260)], 20));
    const end = ev([], 40, [touch(1, 260)]);
    end.cancelable = false;
    gesture.onTouchEnd(end);
    expect(end.prevented).toBe(false);
  });
});

// ghostty's alt-screen wheel path rounds each event's deltaY to whole arrows of
// 33px and discards the rest, so slow drags (small per-move deltas) would emit
// nothing. The quantizer accumulates alt-screen deltas by cell height and emits
// whole-arrow multiples of 33px; normal-screen deltas pass straight through.
describe("createWheelQuantizer", () => {
  function quantizer({ alt = true, cell = 13 } = {}) {
    const emitted = [];
    const state = { alt, cell };
    const dispatch = createWheelQuantizer({
      isAltScreen: () => state.alt,
      getCellHeight: () => state.cell,
      emit: (d) => emitted.push(d),
    });
    return { dispatch, emitted, state };
  }

  it("passes normal-screen deltas through unchanged", () => {
    const { dispatch, emitted, state } = quantizer();
    state.alt = false;
    dispatch(7.5);
    dispatch(-3);
    expect(emitted).toEqual([7.5, -3]);
  });

  it("accumulates alt-screen deltas and emits one 33px arrow per cell height", () => {
    const { dispatch, emitted } = quantizer({ cell: 13 });
    dispatch(5);
    dispatch(5);
    expect(emitted).toEqual([]); // 10px < one 13px cell
    dispatch(5); // 15px ⇒ one line, 2px remainder
    expect(emitted).toEqual([33]);
    dispatch(12); // 14px ⇒ one more line
    expect(emitted).toEqual([33, 33]);
  });

  it("is sign-symmetric for upward scrolling", () => {
    const { dispatch, emitted } = quantizer({ cell: 13 });
    dispatch(-15);
    expect(emitted).toEqual([-33]);
  });

  it("caps a single emission at 5 arrows and carries the remainder", () => {
    const { dispatch, emitted } = quantizer({ cell: 13 });
    dispatch(100); // 7 lines owed ⇒ emit 5, keep 35px
    expect(emitted).toEqual([165]);
    dispatch(5); // 40px ⇒ 3 more lines
    expect(emitted).toEqual([165, 99]);
  });

  it("drops the banked remainder when the screen returns to normal", () => {
    const { dispatch, emitted, state } = quantizer({ cell: 13 });
    dispatch(12); // banked, below one cell
    state.alt = false;
    dispatch(6);
    expect(emitted).toEqual([6]);
    state.alt = true;
    dispatch(12); // old 12px bank must be gone
    expect(emitted).toEqual([6]);
  });
});
