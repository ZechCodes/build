import { afterEach, describe, expect, it, vi } from "vitest";
import { coordinatedRead } from "../src/core/readRequests.js";

const deferred = () => {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
};

const nextTurn = () => new Promise((resolve) => setTimeout(resolve, 0));

afterEach(() => {
  delete globalThis.requestIdleCallback;
});

describe("coordinatedRead", () => {
  it("shares one pending read with every caller of the same key", async () => {
    const answer = deferred();
    const load = vi.fn(() => answer.promise);
    const first = coordinatedRead({ key: "device-a:git.status:run-1", load });
    const second = coordinatedRead({ key: "device-a:git.status:run-1", load });

    expect(second).toBe(first);
    await Promise.resolve();
    expect(load).toHaveBeenCalledTimes(1);
    answer.resolve("status");
    await expect(first).resolves.toBe("status");
  });

  it("forgets a settled read so a later call can refresh it", async () => {
    const load = vi.fn().mockResolvedValueOnce("old").mockResolvedValueOnce("new");
    await expect(coordinatedRead({ key: "refreshable", load })).resolves.toBe("old");
    await expect(coordinatedRead({ key: "refreshable", load })).resolves.toBe("new");
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("forgets a rejected read so a later call can retry it", async () => {
    const load = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce("online");
    await expect(coordinatedRead({ key: "retryable", load })).rejects.toThrow("offline");
    await expect(coordinatedRead({ key: "retryable", load })).resolves.toBe("online");
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("holds background reads until foreground work settles, then yields an idle turn", async () => {
    const idle = [];
    globalThis.requestIdleCallback = vi.fn((callback) => idle.push(callback));
    const foreground = deferred();
    const foregroundRead = coordinatedRead({ key: "foreground", load: () => foreground.promise });
    const backgroundLoad = vi.fn().mockResolvedValue("warm");
    const backgroundRead = coordinatedRead({ key: "background", priority: "background", load: backgroundLoad });

    await Promise.resolve();
    expect(idle).toEqual([]);
    expect(backgroundLoad).not.toHaveBeenCalled();

    foreground.resolve("live");
    await foregroundRead;
    await Promise.resolve();
    expect(idle).toHaveLength(1);
    expect(backgroundLoad).not.toHaveBeenCalled();

    idle.shift()();
    await expect(backgroundRead).resolves.toBe("warm");
  });

  it("promotes a queued background read when a foreground caller joins it", async () => {
    const idle = [];
    globalThis.requestIdleCallback = (callback) => idle.push(callback);
    const load = vi.fn().mockResolvedValue("shared");
    const background = coordinatedRead({ key: "same", priority: "background", load });
    await Promise.resolve();
    const foreground = coordinatedRead({ key: "same", priority: "foreground", load });

    expect(foreground).toBe(background);
    await Promise.resolve();
    expect(load).toHaveBeenCalledTimes(1);
    await expect(foreground).resolves.toBe("shared");
    idle.shift()(); // the background turn was already queued; it cannot load twice
    await Promise.resolve();
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("rechecks foreground activity after a background read yields", async () => {
    const idle = [];
    globalThis.requestIdleCallback = (callback) => idle.push(callback);
    const backgroundLoad = vi.fn().mockResolvedValue("warm");
    const backgroundRead = coordinatedRead({ key: "background-race", priority: "background", load: backgroundLoad });
    await Promise.resolve();

    const foreground = deferred();
    const foregroundRead = coordinatedRead({ key: "foreground-race", load: () => foreground.promise });
    idle.shift()();
    await Promise.resolve();
    expect(backgroundLoad).not.toHaveBeenCalled();

    foreground.resolve("live");
    await foregroundRead;
    await Promise.resolve();
    expect(idle).toHaveLength(1);
    idle.shift()();
    await expect(backgroundRead).resolves.toBe("warm");
  });

  it("uses the next task as the idle turn when requestIdleCallback is unavailable", async () => {
    const load = vi.fn().mockResolvedValue("warm");
    const read = coordinatedRead({ key: "fallback", priority: "background", load });
    await Promise.resolve();
    expect(load).not.toHaveBeenCalled();
    await nextTurn();
    await expect(read).resolves.toBe("warm");
  });
});
