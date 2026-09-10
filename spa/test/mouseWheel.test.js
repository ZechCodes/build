// Wheel → mouse-protocol forwarding (terminal/mouseWheel.js). When the PTY app
// enables mouse tracking (Claude Code does: DECSET 1000 + 1006), wheel input
// must reach it as mouse scroll reports — not the alternate-screen arrow-key
// fallback that makes Claude Code print "Scroll wheel is sending arrow keys".

import { describe, it, expect } from "vitest";
import { wheelReportSequence, createWheelReporter } from "../src/terminal/mouseWheel.js";

describe("wheelReportSequence", () => {
  it("encodes SGR scroll-up as button 64 press", () => {
    expect(wheelReportSequence({ up: true, col: 12, row: 3, sgr: true })).toBe("\x1b[<64;12;3M");
  });

  it("encodes SGR scroll-down as button 65 press", () => {
    expect(wheelReportSequence({ up: false, col: 1, row: 1, sgr: true })).toBe("\x1b[<65;1;1M");
  });

  it("encodes X10 fallback with 32-offset bytes", () => {
    const seq = wheelReportSequence({ up: true, col: 5, row: 2, sgr: false });
    expect(seq).toBe("\x1b[M" + String.fromCharCode(32 + 64, 32 + 5, 32 + 2));
  });

  it("clamps X10 coordinates to the 223 protocol maximum", () => {
    const seq = wheelReportSequence({ up: false, col: 500, row: 0, sgr: false });
    expect(seq.charCodeAt(4)).toBe(32 + 223);
    expect(seq.charCodeAt(5)).toBe(32 + 1);
  });
});

const makeReporter = (overrides = {}) => {
  const sent = [];
  const reporter = createWheelReporter({
    hasMouseTracking: () => true,
    isSgr: () => true,
    getCellSize: () => ({ width: 10, height: 20 }),
    send: (seq) => sent.push(seq),
    ...overrides,
  });
  return { reporter, sent };
};

const wheel = (deltaY, extra = {}) => ({ deltaY, deltaMode: 0, clientX: 105, clientY: 45, ...extra });
const rect = { left: 5, top: 5 };

describe("createWheelReporter", () => {
  it("declines the event (and banks nothing) when the app is not tracking the mouse", () => {
    const { reporter, sent } = makeReporter({ hasMouseTracking: () => false });
    expect(reporter(wheel(300), rect)).toBe(false);
    expect(sent).toEqual([]);
  });

  it("consumes a tracked wheel event even when the delta is below one cell", () => {
    const { reporter, sent } = makeReporter();
    expect(reporter(wheel(6), rect)).toBe(true);
    expect(sent).toEqual([]);
  });

  it("banks sub-cell deltas until a full cell height is crossed", () => {
    const { reporter, sent } = makeReporter();
    reporter(wheel(12), rect);
    reporter(wheel(12), rect); // 24px banked > 20px cell → one report, 4px kept
    expect(sent).toEqual(["\x1b[<65;11;3M"]);
    reporter(wheel(16), rect); // 4 + 16 = 20 → the remainder was carried
    expect(sent.length).toBe(2);
  });

  it("reports scroll-up (negative deltaY) as button 64", () => {
    const { reporter, sent } = makeReporter();
    reporter(wheel(-40), rect);
    expect(sent).toEqual(["\x1b[<64;11;3M", "\x1b[<64;11;3M"]);
  });

  it("caps a single event at five reports", () => {
    const { reporter, sent } = makeReporter();
    reporter(wheel(20 * 9), rect);
    expect(sent.length).toBe(5);
  });

  it("derives the report cell from the pointer position inside the host", () => {
    const { reporter, sent } = makeReporter();
    reporter(wheel(20, { clientX: 5, clientY: 5 }), rect); // top-left corner → 1;1
    expect(sent).toEqual(["\x1b[<65;1;1M"]);
  });

  it("treats line-mode deltas as whole cells", () => {
    const { reporter, sent } = makeReporter();
    reporter(wheel(3, { deltaMode: 1 }), rect);
    expect(sent.length).toBe(3);
  });

  it("falls back to X10 encoding when SGR mode is off", () => {
    const { reporter, sent } = makeReporter({ isSgr: () => false });
    reporter(wheel(20), rect);
    expect(sent).toEqual(["\x1b[M" + String.fromCharCode(32 + 65, 32 + 11, 32 + 3)]);
  });

  it("drops the bank when tracking turns off between events", () => {
    let tracking = true;
    const { reporter, sent } = makeReporter({ hasMouseTracking: () => tracking });
    reporter(wheel(12), rect);
    tracking = false;
    reporter(wheel(12), rect);
    tracking = true;
    reporter(wheel(12), rect); // a fresh bank: 12px < 20px → nothing yet
    expect(sent).toEqual([]);
  });
});
