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
    const onDevice = { name: "issue", deviceId: "dev-1", projectId: "p1", id: "i-1" };
    markRoute(onDevice);

    expect(App.route).toBe(onDevice);
    expect(location.hash).toBe("#/device/dev-1/project/p1/issue/i-1");
  });
});
