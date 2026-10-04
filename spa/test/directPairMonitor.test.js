import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDirectPairMonitor } from "../src/core/directPairMonitor.js";

describe("direct-pair observation budget", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
  });
  afterEach(() => vi.useRealTimers());

  it("backs off viability samples from five seconds to one minute before an attempt", async () => {
    const sampled = [];
    const monitor = createDirectPairMonitor({ check: () => sampled.push(Date.now()), canCheck: () => true });
    monitor.start();
    await vi.advanceTimersByTimeAsync(215000);
    expect(sampled).toEqual([20000, 25000, 35000, 55000, 95000, 155000, 215000]);
    monitor.close();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("continues sparse observation for evidence arriving long after the first sample", async () => {
    let available = false;
    let attempted = false;
    const check = vi.fn(() => { if (available) attempted = true; });
    const monitor = createDirectPairMonitor({ check, canCheck: () => true, hasAttempted: () => attempted });
    monitor.start();
    await vi.advanceTimersByTimeAsync(600000);
    expect(attempted).toBe(false);
    available = true;
    await vi.advanceTimersByTimeAsync(60000);
    expect(attempted).toBe(true);
    monitor.close();
  });

  it("starts its two-minute post-attempt window when the attempt finishes and never renews it", async () => {
    let finishAttempt;
    let attempted = false;
    const sampled = [];
    const check = vi.fn(() => {
      sampled.push(Date.now());
      if (attempted) return;
      attempted = true;
      return new Promise((resolve) => { finishAttempt = resolve; });
    });
    const monitor = createDirectPairMonitor({ check, canCheck: () => true, hasAttempted: () => attempted });
    monitor.start();
    await vi.advanceTimersByTimeAsync(35000);
    expect(sampled).toEqual([20000]);
    finishAttempt();
    await vi.advanceTimersByTimeAsync(120000);
    expect(sampled).toEqual([20000, 40000, 50000, 70000, 110000, 155000]);
    expect(vi.getTimerCount()).toBe(0);
    monitor.start();
    await vi.advanceTimersByTimeAsync(600000);
    expect(sampled).toHaveLength(6);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("preserves a recovery's settling deadline when start arrives during an awaited sample", async () => {
    let finish;
    const sampled = [];
    const check = vi.fn(() => {
      sampled.push(Date.now());
      if (sampled.length === 1) return new Promise((resolve) => { finish = resolve; });
    });
    const monitor = createDirectPairMonitor({ check, canCheck: () => true });
    monitor.start();
    await vi.advanceTimersByTimeAsync(23000);
    monitor.start();
    await vi.advanceTimersByTimeAsync(7000);
    finish();
    await vi.advanceTimersByTimeAsync(12999);
    expect(sampled).toEqual([20000]);
    await vi.advanceTimersByTimeAsync(5001);
    expect(sampled).toEqual([20000, 43000, 48000]);
    monitor.close();
  });

  it("preserves a pending start without renewing the spent attempt's observation deadline", async () => {
    let finish;
    let attempted = false;
    const sampled = [];
    const check = vi.fn(() => {
      sampled.push(Date.now());
      attempted = true;
      if (sampled.length === 2) return new Promise((resolve) => { finish = resolve; });
    });
    const monitor = createDirectPairMonitor({ check, canCheck: () => true, hasAttempted: () => attempted });
    monitor.start();
    await vi.advanceTimersByTimeAsync(35000);
    monitor.start();
    finish();
    await vi.advanceTimersByTimeAsync(120000);
    expect(sampled).toEqual([20000, 25000, 55000, 65000, 85000, 125000, 140000]);
    expect(vi.getTimerCount()).toBe(0);
    monitor.start();
    await vi.advanceTimersByTimeAsync(600000);
    expect(sampled).toHaveLength(7);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("skips an observation whose timer fires after the bounded window elapsed", async () => {
    let wallClock = 0;
    const check = vi.fn();
    const monitor = createDirectPairMonitor({
      check, canCheck: () => true, hasAttempted: () => true, now: () => wallClock,
    });
    monitor.start();
    await vi.advanceTimersByTimeAsync(20000);
    expect(check).toHaveBeenCalledTimes(1);
    wallClock = 180000;
    await vi.advanceTimersByTimeAsync(5000);
    expect(check).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("leaves no timers after closing an awaited sample", async () => {
    let finish;
    const monitor = createDirectPairMonitor({
      check: () => new Promise((resolve) => { finish = resolve; }), canCheck: () => true,
    });
    monitor.start();
    await vi.advanceTimersByTimeAsync(20000);
    monitor.close();
    finish();
    await vi.advanceTimersByTimeAsync(600000);
    expect(vi.getTimerCount()).toBe(0);
  });
});
