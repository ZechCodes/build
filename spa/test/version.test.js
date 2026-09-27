// The frontend version watcher: the served bundle knows the version it was
// built as, the server publishes the version it is currently serving
// (version.json, emitted by the build), and a client that finds itself behind
// offers one reload. The check runs on an interval AND on visibility resume —
// a suspended PWA misses any push a deploy could send, so waking up IS the
// moment it must compare.

import { describe, expect, it, vi } from "vitest";
import { createVersionWatcher } from "../src/core/version.js";

function harness({ current = "aaa", served = "bbb" } = {}) {
  const onStale = vi.fn();
  const fetchVersion = vi.fn().mockResolvedValue(served);
  const listeners = {};
  const documentLike = {
    visibilityState: "visible",
    addEventListener: (name, fn) => (listeners[name] = fn),
  };
  const watcher = createVersionWatcher({
    currentVersion: current,
    fetchVersion,
    onStale,
    intervalMs: 60_000,
    documentLike,
    setIntervalImpl: vi.fn().mockReturnValue(1),
  });
  return { watcher, onStale, fetchVersion, listeners, documentLike };
}

describe("createVersionWatcher", () => {
  it("fires onStale once when the served version differs", async () => {
    const { watcher, onStale } = harness({ current: "aaa", served: "bbb" });
    await watcher.check();
    await watcher.check();
    expect(onStale).toHaveBeenCalledTimes(1);
  });

  it("stays quiet while the served version matches", async () => {
    const { watcher, onStale } = harness({ current: "aaa", served: "aaa" });
    await watcher.check();
    expect(onStale).not.toHaveBeenCalled();
  });

  it("ignores fetch failures and keeps watching", async () => {
    const { watcher, onStale, fetchVersion } = harness();
    fetchVersion.mockRejectedValueOnce(new Error("offline"));
    await watcher.check();
    expect(onStale).not.toHaveBeenCalled();
    await watcher.check(); // next check still runs and now sees the mismatch
    expect(onStale).toHaveBeenCalledTimes(1);
  });

  it("checks when the document becomes visible again", async () => {
    const { watcher, onStale, listeners } = harness();
    watcher.start();
    await listeners.visibilitychange();
    expect(onStale).toHaveBeenCalledTimes(1);
  });

  it("does not check while the document stays hidden", async () => {
    const { watcher, onStale, listeners, documentLike } = harness();
    watcher.start();
    documentLike.visibilityState = "hidden";
    await listeners.visibilitychange();
    expect(onStale).not.toHaveBeenCalled();
  });

  it("never starts for a dev build", () => {
    const setIntervalImpl = vi.fn();
    const documentLike = { visibilityState: "visible", addEventListener: vi.fn() };
    const watcher = createVersionWatcher({
      currentVersion: "dev",
      fetchVersion: vi.fn(),
      onStale: vi.fn(),
      intervalMs: 60_000,
      documentLike,
      setIntervalImpl,
    });
    watcher.start();
    expect(setIntervalImpl).not.toHaveBeenCalled();
    expect(documentLike.addEventListener).not.toHaveBeenCalled();
  });
});

describe("the build's version stamp", () => {
  it("emits version.json naming the version the bundle embeds", async () => {
    const { versionStampPlugin } = await import("../vite.config.js");
    const emitted = [];
    versionStampPlugin("sha-123").generateBundle.call({ emitFile: (f) => emitted.push(f) });
    expect(emitted).toEqual([
      { type: "asset", fileName: "version.json", source: JSON.stringify({ version: "sha-123" }) },
    ]);
  });
});
