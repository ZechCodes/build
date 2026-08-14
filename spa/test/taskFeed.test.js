// @vitest-environment jsdom
// The shared feed snapshot normalizes the wire's shapes: the daemon's
// project.list rows carry `project_id`, while every consumer of the snapshot
// (the toolbar's scope, its menu, the project names) reads `id`. The feed is
// the one place the wire is read, so it is the one place the key is bridged.

import { describe, it, expect, beforeEach, vi } from "vitest";

let App;
let subscribeFeed, refreshFeed;

beforeEach(async () => {
  vi.resetModules();
  ({ App } = await import("../src/app.js"));
  ({ subscribeFeed, refreshFeed } = await import("../src/core/taskFeed.js"));
});

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
