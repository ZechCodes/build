// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { CHAT_LANDING_READ_WAIT_MS, createChatLanding } from "../src/core/chatLanding.js";

afterEach(() => vi.useRealTimers());
const paint = (landing, body, extra = {}) => landing.prepare(body, { hasItems: true, target: null, waitingForHistory: false, ...extra });

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
});
