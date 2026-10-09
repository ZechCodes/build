import { afterEach, describe, expect, it, vi } from "vitest";
import { coordinatedRead, requestPriorityFields } from "../src/core/readRequests.js";

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

  it.each(["foreground", "background"])("replaces a stopped queued read for a new %s caller", async (priority) => {
    const idle = [];
    globalThis.requestIdleCallback = (callback) => idle.push(callback);
    const key = `stopped-queued-${priority}`;
    let active = true;
    const stopped = coordinatedRead({
      key,
      priority: "background",
      active: () => active,
      load: () => active ? "old" : null,
    });
    await Promise.resolve();
    active = false;

    const load = vi.fn().mockResolvedValue("fresh");
    const replacement = coordinatedRead({ key, priority, load });
    await Promise.resolve();
    if (priority === "foreground") {
      await replacement;
    }
    for (const callback of idle.splice(0)) callback();
    const [stoppedAnswer, replacementAnswer] = await Promise.all([stopped, replacement]);

    expect(replacement).not.toBe(stopped);
    expect(stoppedAnswer).toBeNull();
    expect(replacementAnswer).toBe("fresh");
    expect(load).toHaveBeenCalledExactlyOnceWith(requestPriorityFields(priority));
  });

  it("uses the active caller's loader when the original reader stops after promotion", async () => {
    const idle = [];
    globalThis.requestIdleCallback = (callback) => idle.push(callback);
    let active = true;
    const stoppedLoad = vi.fn(() => active ? "old" : null);
    const stopped = coordinatedRead({
      key: "stopped-after-promotion",
      priority: "background",
      active: () => active,
      load: stoppedLoad,
    });
    await Promise.resolve();

    const load = vi.fn().mockResolvedValue("fresh");
    const joined = coordinatedRead({ key: "stopped-after-promotion", load });
    active = false;
    const answer = await joined;
    idle.shift()();

    expect(joined).toBe(stopped);
    expect(answer).toBe("fresh");
    expect(stoppedLoad).not.toHaveBeenCalled();
    expect(load).toHaveBeenCalledExactlyOnceWith({});
  });

  it("sends no read when all joined readers stop before dispatch", async () => {
    const idle = [];
    globalThis.requestIdleCallback = (callback) => idle.push(callback);
    let active = true;
    const load = vi.fn().mockResolvedValue("fresh");
    const request = { key: "all-stopped", active: () => active, load };
    const first = coordinatedRead({ ...request, priority: "background" });
    await Promise.resolve();
    const joined = coordinatedRead(request);
    active = false;
    const answer = await joined;
    idle.shift()();

    expect(joined).toBe(first);
    expect(answer).toBeNull();
    expect(load).not.toHaveBeenCalled();
  });

  it("keeps the replacement pending when a stopped read settles", async () => {
    const previous = deferred();
    const fresh = deferred();
    let active = true;
    const key = "stopped-in-flight";
    const stopped = coordinatedRead({ key, active: () => active, load: () => previous.promise });
    await Promise.resolve();
    active = false;
    const replacement = coordinatedRead({ key, load: () => fresh.promise });
    previous.resolve("old");
    await stopped;

    const unwanted = vi.fn().mockResolvedValue("duplicate");
    const joined = coordinatedRead({ key, load: unwanted });
    fresh.resolve("fresh");
    const answers = await Promise.all([replacement, joined]);

    expect(joined).toBe(replacement);
    expect(answers).toEqual(["fresh", "fresh"]);
    expect(unwanted).not.toHaveBeenCalled();
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

describe("the request envelope's priority", () => {
  it("stamps background onto the envelope and leaves foreground bare", () => {
    expect(requestPriorityFields("background")).toEqual({ priority: "background" });
    expect(requestPriorityFields("foreground")).toEqual({});
    expect(requestPriorityFields(undefined)).toEqual({});
  });

  it("hands each load the envelope fields its own priority rides with", async () => {
    const foregroundLoad = vi.fn().mockResolvedValue("live");
    await coordinatedRead({ key: "envelope-foreground", load: foregroundLoad });
    expect(foregroundLoad).toHaveBeenCalledWith({});

    const backgroundLoad = vi.fn().mockResolvedValue("warm");
    await coordinatedRead({ key: "envelope-background", priority: "background", load: backgroundLoad });
    expect(backgroundLoad).toHaveBeenCalledWith({ priority: "background" });
  });
});
