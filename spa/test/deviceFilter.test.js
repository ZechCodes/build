// @vitest-environment jsdom
// The rail's device filter: which machines the inbox shows.
//
// It is a snapshot-to-snapshot function whose "all devices" case is the
// identity, so no list painter branches on whether a filter is set — each one
// wraps the snapshot it was handed and paints whatever comes back. Remembering
// a pick is the other half: it is kept on this browser, and every surface that
// paints a list is handed the merge again so it repaints through the path it
// already has.

import { describe, it, expect, beforeEach, vi } from "vitest";

const subscribers = vi.hoisted(() => new Set());
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscribers.add(fn);
    return () => subscribers.delete(fn);
  },
  deliverFeed: () => subscribers.forEach((fn) => fn(merged)),
  dropFeedDevice: () => {},
  primaryRunIdFor: () => null,
  refreshFeed: () => {},
  startFeed: () => {},
  stopFeed: () => {},
}));

const { App } = await import("../src/app.js");
const { filterByDevice, rememberDeviceFilter } = await import("../src/core/deviceFilter.js");

const row = (deviceId, id) => ({ id, deviceId, projectKey: `${deviceId}/p1`, project_id: "p1" });

const viewFor = (deviceId) => ({
  items: [row(deviceId, `${deviceId}-item`)],
  plans: [row(deviceId, `${deviceId}-plan`)],
  runs: [row(deviceId, `${deviceId}-run`)],
  externalWorktrees: [row(deviceId, `${deviceId}-wt`)],
  pending: [row(deviceId, `${deviceId}-pending`)],
  primaryChanges: [row(deviceId, `${deviceId}-primary`)],
  projects: [row(deviceId, `${deviceId}-project`)],
});

const COLLECTIONS = Object.keys(viewFor("dev-a"));

/** Two machines' snapshots as the feed merges them. */
function merge(cached = false) {
  const mine = viewFor("dev-a");
  const theirs = viewFor("dev-b");
  const both = Object.fromEntries(COLLECTIONS.map((field) => [field, [...mine[field], ...theirs[field]]]));
  return { ...both, devices: { "dev-a": mine, "dev-b": theirs }, cached };
}

let merged = merge();

beforeEach(() => {
  merged = merge();
  subscribers.clear();
  localStorage.clear();
  App.deviceFilter = null;
});

describe("filtering the merge to one machine", () => {
  it("answers the same snapshot for no filter", () => {
    expect(filterByDevice(merged, null)).toBe(merged);
  });

  it("keeps only one device's rows in every collection, and only its entry under devices", () => {
    const shown = filterByDevice(merged, "dev-b");

    for (const field of COLLECTIONS) {
      expect(shown[field].map((entry) => entry.deviceId), field).toEqual(["dev-b"]);
    }
    expect(Object.keys(shown.devices)).toEqual(["dev-b"]);
    expect(shown.devices["dev-b"]).toBe(merged.devices["dev-b"]);
    // …and the merge it was made from is untouched: other subscribers read it.
    expect(merged.items).toHaveLength(2);
  });

  it("carries cached over", () => {
    expect(filterByDevice(merge(true), "dev-a").cached).toBe(true);
    expect(filterByDevice(merge(false), "dev-a").cached).toBe(false);
  });
});

describe("remembering which machines the inbox shows", () => {
  it("rememberDeviceFilter writes App.deviceFilter and build.deviceFilter, null removes the key, and every feed subscriber is handed the merge again", () => {
    const painted = [];
    subscribers.add((snapshot) => painted.push(snapshot));

    rememberDeviceFilter("dev-b");

    expect(App.deviceFilter).toBe("dev-b");
    expect(localStorage.getItem("build.deviceFilter")).toBe("dev-b");
    expect(painted).toHaveLength(1);

    rememberDeviceFilter(null);

    expect(App.deviceFilter).toBe(null);
    expect(localStorage.getItem("build.deviceFilter")).toBe(null);
    expect(painted).toHaveLength(2);
  });
});
