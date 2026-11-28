// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { OPENING_READ_WAIT_MS, createOpeningPosition } from "../src/core/openingPosition.js";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
const paint = (landing, body, extra = {}) => {
  Object.defineProperty(body, "clientHeight", { configurable: true, value: 300 });
  return landing.prepare(body, { hasItems: true, target: null, waitingForHistory: false, ...extra });
};

describe("the opening landing's bounded read wait", () => {
  it("releases reports after five seconds even when the cursor/history never arrives", () => {
    vi.useFakeTimers();
    const resumed = vi.fn();
    const landing = createOpeningPosition(resumed);
    const body = document.createElement("div");
    const visit = paint(landing, body, { waitingForHistory: true });
    visit.onLand(false);
    visit.onLand(true);
    expect(landing.waiting()).toBe(true);
    vi.advanceTimersByTime(OPENING_READ_WAIT_MS - 1);
    expect(resumed).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(OPENING_READ_WAIT_MS).toBe(5000);
    expect(landing.waiting()).toBe(false);
    expect(resumed).toHaveBeenCalledWith(body);
    landing.dispose();
  });

  it("lets the late divider land after the read wait has expired", () => {
    vi.useFakeTimers();
    const landing = createOpeningPosition(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body, { waitingForHistory: true });
    first.onLand(false); first.onLand(true);
    vi.advanceTimersByTime(OPENING_READ_WAIT_MS);
    const late = paint(landing, body, { target: 12 });
    expect(late.opening).toBe(true);
    expect(landing.waiting()).toBe(false);
    landing.dispose();
  });

  it("keeps a settled visit closed when New expires or a new unread burst arrives", () => {
    const landing = createOpeningPosition(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body, { target: 12 });
    first.onLand(false); first.onLand(true);
    expect(landing.active()).toBe(false);
    expect(paint(landing, body, { target: null }).opening).toBe(false);
    expect(paint(landing, body, { target: 19 }).opening).toBe(false);
    landing.dispose();
  });

  it("settles the same provisional anchor when an activity-only history page completes coverage", () => {
    const landing = createOpeningPosition(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body, { target: 12, waitingForHistory: true });
    first.onLand(false); first.onLand(true);
    const final = paint(landing, body, { target: 12, waitingForHistory: false });
    expect(final.opening).toBe(true);
    final.onLand(false); final.onLand(true);
    expect(landing.active()).toBe(false);
    landing.dispose();
  });

  it("does not mistake programmatic history anchoring for reader motion", () => {
    const landing = createOpeningPosition(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body, { target: 12, waitingForHistory: true });
    first.onLand(false); first.onLand(true);
    body.scrollTop = 100;
    landing.painted();
    body.dispatchEvent(new Event("scroll"));
    expect(landing.active()).toBe(true);
    expect(paint(landing, body, { target: 11 }).opening).toBe(true);
    landing.dispose();
  });

  it("cancels stale frame corrections on switches and disposal", () => {
    const landing = createOpeningPosition(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body);
    landing.reset();
    expect(first.canLand()).toBe(false);
    const second = paint(landing, body);
    landing.dispose();
    expect(second.canLand()).toBe(false);
  });

  it.each(["reset", "dispose"])("%s disconnects resize tracking and protects the next view", (endVisit) => {
    const observed = new Set();
    const disconnect = vi.fn(() => observed.clear());
    let resize;
    vi.stubGlobal("ResizeObserver", class {
      constructor(onResize) {
        this.disconnect = disconnect;
        resize = (target) => { if (observed.has(target)) onResize(); };
      }
      observe(target) { observed.add(target); }
    });
    const landing = createOpeningPosition(() => {});
    const body = document.createElement("div");
    const oldTimeline = body.appendChild(document.createElement("section"));
    const previousPosition = vi.fn(() => {
      body.scrollTop = 250;
      return true;
    });
    paint(landing, body).onLand(true, previousPosition);
    landing.painted(oldTimeline);
    expect(observed).toEqual(new Set([body, oldTimeline]));
    previousPosition.mockClear();
    resize(oldTimeline);
    expect(previousPosition).toHaveBeenCalledOnce();
    previousPosition.mockClear();

    landing[endVisit]();
    body.replaceChildren(document.createElement("section"));
    body.scrollTop = 40;
    resize(body);
    expect(body.scrollTop).toBe(40);
    expect(previousPosition).not.toHaveBeenCalled();
    expect(disconnect).toHaveBeenCalledOnce();
    expect(observed.size).toBe(0);
    if (endVisit === "reset") landing.dispose();
  });

  it("reader motion abandons the landing and releases its report wait", () => {
    const resumed = vi.fn();
    const landing = createOpeningPosition(resumed);
    const body = document.createElement("div");
    const first = paint(landing, body, { waitingForHistory: true });
    first.onLand(false);
    body.dispatchEvent(new WheelEvent("wheel"));
    expect(first.canLand()).toBe(false);
    expect(landing.waiting()).toBe(false);
    expect(paint(landing, body, { target: 12 }).opening).toBe(false);
    landing.dispose();
  });

  it.each(["pointerdown", "touchstart", "wheel", "keydown"])(
    "%s resumes reporting when reader input produces no scroll", (type) => {
      vi.useFakeTimers();
      const resumed = vi.fn();
      const landing = createOpeningPosition(resumed);
      const body = document.createElement("div");
      const first = paint(landing, body, { waitingForHistory: true });
      first.onLand(false);
      first.onLand(true);
      const event = type === "keydown"
        ? new KeyboardEvent(type, { key: "ArrowUp" })
        : new Event(type);
      body.dispatchEvent(event);
      expect(body.scrollTop).toBe(0);
      expect(first.canLand()).toBe(false);
      expect(landing.waiting()).toBe(false);
      expect(resumed).toHaveBeenCalledExactlyOnceWith(body);
      vi.advanceTimersByTime(OPENING_READ_WAIT_MS);
      expect(resumed).toHaveBeenCalledTimes(1);
      landing.dispose();
    },
  );

  it.each(["Enter", " ", "click"])("releases the opening before a transcript control handles %j", (activation) => {
    const landing = createOpeningPosition(() => {});
    const body = document.createElement("div");
    const button = body.appendChild(document.createElement("button"));
    const visit = paint(landing, body, { waitingForHistory: true });
    const type = activation === "click" ? "click" : "keydown";
    const handled = vi.fn((event) => {
      event.stopPropagation();
      return visit.canLand();
    });
    button.addEventListener(type, handled);
    if (type === "click") button.click();
    else button.dispatchEvent(new KeyboardEvent(type, { key: activation, bubbles: true }));
    expect(handled).toHaveBeenCalledOnce();
    expect(handled).toHaveReturnedWith(false);
    expect(landing.waiting()).toBe(false);
    landing.dispose();
  });
});
