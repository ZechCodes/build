// @vitest-environment jsdom
// Nothing can land on a work surface without saying which machine the work is
// on: every device mints a `proj-1`. A producer that builds a route by hand and
// forgets is not trusted with it — `go` and `markRoute` park such a route on the
// resolve hop, and the URL keeps the link exactly as it was written so reading
// it back asks the same question.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { App, go, markRoute } from "../src/app.js";

const deviceLess = { name: "branch", projectId: "p1", branch: "main", tab: "changes" };
const parked = { name: "resolve", kind: "project", projectId: "p1", route: deviceLess };

beforeEach(() => {
  history.replaceState(null, "", "#/inbox");
  App.route = { name: "inbox" };
  App.routeLeaveGuard = null;
  App.viewingContext = { clear: vi.fn() };
});

describe("a route that names no device", () => {
  it("is parked on the resolve hop by go, with the link written as it was given", () => {
    expect(go(deviceLess)).toBe(true);

    expect(App.route).toEqual(parked);
    // Written from the route as given: the parked route's own hash is the
    // inbox, and writing that would send every device-less link there.
    expect(location.hash).toBe("#/project/p1/branch/main/changes");
  });

  it("is parked on the resolve hop by markRoute too", () => {
    markRoute({ name: "branch", projectId: "p1", branch: "main", tab: "files", file: "a/b" });

    expect(App.route).toEqual({
      name: "resolve", kind: "project", projectId: "p1",
      route: { name: "branch", projectId: "p1", branch: "main", tab: "files", file: "a/b" },
    });
    expect(location.hash).toBe("#/project/p1/branch/main/files?path=a%2Fb");
  });
});

describe("a route that names its device", () => {
  it("adds exactly one history entry when a destination picker opts in", () => {
    const entries = history.length;
    const destination = { name: "workspace", deviceId: "dev-2", projectId: "p2", workspaceId: "w2", tab: "files" };
    expect(go(destination, { newEntry: true })).toBe(true);
    expect(history.length).toBe(entries + 1);
    expect(App.route).toEqual(destination);
  });

  it("adds no history entry when a guarded picker selection is rejected", async () => {
    const entries = history.length;
    App.routeLeaveGuard = vi.fn(async () => false);
    expect(await go({ name: "project", deviceId: "dev-2", projectId: "p2" }, { newEntry: true })).toBe(false);
    expect(history.length).toBe(entries);
    expect(location.hash).toBe("#/inbox");
  });

  it("adds one entry for only the winning guarded picker selection", async () => {
    const entries = history.length;
    let decide;
    App.routeLeaveGuard = () => new Promise((resolve) => { decide = resolve; });
    const first = go({ name: "project", deviceId: "dev-2", projectId: "p2" }, { newEntry: true });
    const latest = go({ name: "project", deviceId: "dev-2", projectId: "p3" }, { newEntry: true });
    decide(true);
    expect(await first).toBe(false);
    expect(await latest).toBe(true);
    expect(history.length).toBe(entries + 1);
    expect(App.route.projectId).toBe("p3");
  });

  it("replaces the current history entry when navigating between surfaces", () => {
    const entries = history.length;
    go({ name: "account" });
    go({ name: "inbox" });
    go({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" });

    expect(history.length).toBe(entries);
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/main/changes");
  });

  it("goes straight to the surface it names", () => {
    const onDevice = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" };
    expect(go(onDevice)).toBe(true);

    expect(App.route).toBe(onDevice);
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/main/changes");
  });

  it("is left alone by markRoute", () => {
    const onDevice = { name: "task", deviceId: "dev-1", projectId: "p1", id: "i-1" };
    markRoute(onDevice);

    expect(App.route).toBe(onDevice);
    expect(location.hash).toBe("#/device/dev-1/project/p1/plan/i-1");
  });
});
