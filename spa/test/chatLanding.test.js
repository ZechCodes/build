// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { CHAT_LANDING_READ_WAIT_MS, createChatLanding } from "../src/core/chatLanding.js";

afterEach(() => vi.useRealTimers());
const paint = (landing, body, extra = {}) => {
  Object.defineProperty(body, "clientHeight", { configurable: true, value: 300 });
  return landing.prepare(body, { hasItems: true, target: null, waitingForHistory: false, ...extra });
};

describe("the opening landing's bounded read wait", () => {
  it("releases reports after five seconds even when the cursor/history never arrives", () => {
    vi.useFakeTimers();
    const resumed = vi.fn();
    const landing = createChatLanding(resumed);
    const body = document.createElement("div");
    const visit = paint(landing, body, { waitingForHistory: true });
    visit.onLand(false);
    visit.onLand(true);
    expect(landing.waiting()).toBe(true);
    vi.advanceTimersByTime(CHAT_LANDING_READ_WAIT_MS - 1);
    expect(resumed).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(CHAT_LANDING_READ_WAIT_MS).toBe(5000);
    expect(landing.waiting()).toBe(false);
    expect(resumed).toHaveBeenCalledWith(body);
    landing.dispose();
  });

  it("lets the late divider land after the read wait has expired", () => {
    vi.useFakeTimers();
    const landing = createChatLanding(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body, { waitingForHistory: true });
    first.onLand(false); first.onLand(true);
    vi.advanceTimersByTime(CHAT_LANDING_READ_WAIT_MS);
    const late = paint(landing, body, { target: 12 });
    expect(late.opening).toBe(true);
    expect(landing.waiting()).toBe(false);
    landing.dispose();
  });

  it("keeps a settled visit closed when New expires or a new unread burst arrives", () => {
    const landing = createChatLanding(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body, { target: 12 });
    first.onLand(false); first.onLand(true);
    expect(landing.active()).toBe(false);
    expect(paint(landing, body, { target: null }).opening).toBe(false);
    expect(paint(landing, body, { target: 19 }).opening).toBe(false);
    landing.dispose();
  });

  it("settles the same provisional anchor when an activity-only history page completes coverage", () => {
    const landing = createChatLanding(() => {});
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
    const landing = createChatLanding(() => {});
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
    const landing = createChatLanding(() => {});
    const body = document.createElement("div");
    const first = paint(landing, body);
    landing.reset();
    expect(first.canLand()).toBe(false);
    const second = paint(landing, body);
    landing.dispose();
    expect(second.canLand()).toBe(false);
  });

  it("reader motion abandons the landing and releases its report wait", () => {
    const resumed = vi.fn();
    const landing = createChatLanding(resumed);
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
      const landing = createChatLanding(resumed);
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
      vi.advanceTimersByTime(CHAT_LANDING_READ_WAIT_MS);
      expect(resumed).toHaveBeenCalledTimes(1);
      landing.dispose();
    },
  );
});
