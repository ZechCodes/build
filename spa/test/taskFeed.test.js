// @vitest-environment jsdom
// The shared feed snapshot normalizes the wire's shapes: the daemon's
// project.list rows carry `project_id`, while every consumer of the snapshot
// (the toolbar's scope, its menu, the project names) reads `id`. The feed is
// the one place the wire is read, so it is the one place the key is bridged.

import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";

let App;
let subscribeFeed, refreshFeed, startFeed, stopFeed;
let armChangeEvents, dispatchChangeEvent, resetChangeEvents, SAFETY_POLL_MS;

beforeEach(async () => {
  vi.resetModules();
  ({ App } = await import("../src/app.js"));
  ({ subscribeFeed, refreshFeed, startFeed, stopFeed } = await import("../src/core/taskFeed.js"));
  ({ armChangeEvents, dispatchChangeEvent, resetChangeEvents, SAFETY_POLL_MS } = await import(
    "../src/core/changeEvents.js"
  ));
});

afterEach(() => {
  stopFeed();
  resetChangeEvents();
  vi.useRealTimers();
});

/** The feed's reads, counted: one tick is a board.list and a project.list. */
const countingCall = () => {
  const call = vi.fn(async (method) =>
    method === "project.list" ? { projects: [] } : { items: [] },
  );
  App.call = call;
  return () => call.mock.calls.filter(([method]) => method === "board.list").length;
};

/** startFeed's own first read, awaited, so a test counts only what follows. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("the shared feed", () => {
  it("gives every project row the id consumers read, from the wire's project_id", async () => {
    App.call = vi.fn(async (method) =>
      method === "project.list"
        ? { projects: [{ project_id: "proj-1", name: "relaydb", path: "/r" }] }
        : { items: [] },
    );
    let snapshot = null;
    subscribeFeed((feed) => {
      snapshot = feed;
    });
    await refreshFeed();
    expect(snapshot.projects).toEqual([
      { id: "proj-1", project_id: "proj-1", name: "relaydb", path: "/r" },
    ]);
  });
});

describe("the feed against a bridge that pushes", () => {
  it("reads once per board.changed, and no more", async () => {
    const reads = countingCall();
    armChangeEvents({ push_events: true });
    startFeed();
    await settle();
    expect(reads()).toBe(1); // startFeed's own first read

    dispatchChangeEvent({ type: "board.changed" });
    await settle();
    expect(reads()).toBe(2);
  });

  it("ignores an entity's event — the board's own event covers the feed", async () => {
    const reads = countingCall();
    armChangeEvents({ push_events: true });
    startFeed();
    await settle();
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" });
    await settle();
    expect(reads()).toBe(1);
  });

  it("stands its fast poll down to the safety poll", async () => {
    vi.useFakeTimers();
    const reads = countingCall();
    armChangeEvents({ push_events: true });
    startFeed(2000);
    await vi.advanceTimersByTimeAsync(SAFETY_POLL_MS - 1000);
    expect(reads()).toBe(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(reads()).toBe(2);
  });
});

describe("the feed against a bridge that does not", () => {
  it("keeps polling at its own cadence", async () => {
    vi.useFakeTimers();
    const reads = countingCall();
    startFeed(2000);
    await vi.advanceTimersByTimeAsync(2000);
    expect(reads()).toBe(2);
    await vi.advanceTimersByTimeAsync(2000);
    expect(reads()).toBe(3);
  });

  it("does not read on a change event it was never told to expect", async () => {
    const reads = countingCall();
    startFeed();
    await settle();
    dispatchChangeEvent({ type: "board.changed" });
    await settle();
    expect(reads()).toBe(1);
  });
});
