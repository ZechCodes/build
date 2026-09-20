// @vitest-environment jsdom
// Where a URL that does not say enough waits. A pre-redesign URL names a run, a
// worktree, a plan or a primary checkout by id; a work URL with no device in it
// names a project every machine has its own `proj-1` of. Either way the answer
// is in the feed — once the devices that can answer have.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

let feedSnapshot = null;
let subscriber = null;
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscriber = fn;
    if (feedSnapshot) fn(feedSnapshot);
    return () => {
      subscriber = null;
    };
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => [],
  dropFeedDevice: () => {},
}));

const { App } = await import("../src/app.js");
const { routeFromHash } = await import("../src/core/router.js");
const { renderResolving } = await import("../src/views/resolving.js");
const contexts = await import("../src/core/deviceContexts.js");

// The same legacy worktree id on two machines: both mint a `p1`, so a URL
// carrying that id names a checkout in each of them and the account has to be
// asked which one the reader meant.
const checkoutRow = (deviceId, branch) => ({
  kind: "branch",
  project_id: "p1",
  branch,
  worktree_id: "wt-1",
  deviceId,
});

/** A device with an open session, the way the connection registers one. */
const openSession = (deviceId) =>
  contexts.adoptDeviceSession({
    deviceId,
    call: async () => ({}),
    close: () => {},
    peer: () => {},
    onCarrier: () => {},
  });

/** A poll answering, the way the feed delivers one: this snapshot, to whoever
 *  the surface subscribed with. */
function deliverFeed(snapshot) {
  feedSnapshot = snapshot;
  subscriber?.(snapshot);
}

/** A merged snapshot holding exactly these devices' views. */
function merge(views) {
  const entries = Object.entries(views);
  return {
    items: entries.flatMap(([, view]) => view.items || []),
    projects: entries.flatMap(([, view]) => view.projects || []),
    devices: Object.fromEntries(entries.map(([deviceId, view]) => [deviceId, { projects: [], ...view }])),
  };
}

beforeEach(() => {
  feedSnapshot = null;
  subscriber = null;
  document.body.innerHTML = bodyHtml;
  location.hash = "";
  App.devices = [
    { id: "dev-2", name: "Desktop", status: "online" },
    { id: "dev-1", name: "This device", status: "online" },
  ];
  App.selectedDeviceId = "dev-1";
});

afterEach(() => {
  contexts.resetDeviceContexts();
  App.devices = [];
  App.selectedDeviceId = null;
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
});

describe("a legacy URL on an account with more than one device", () => {
  it("opens the home device's checkout, not the one that sorts first", async () => {
    // The desktop's own `p1` sorts first in the merge.
    openSession("dev-1");
    openSession("dev-2");
    const theirs = checkoutRow("dev-2", "their-main");
    const mine = checkoutRow("dev-1", "main");
    feedSnapshot = merge({ "dev-2": { items: [theirs] }, "dev-1": { items: [mine] } });
    App.route = { name: "resolve", kind: "worktree", projectId: "p1", id: "wt-1", tab: "changes" };

    renderResolving();

    expect(App.route).toEqual({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" });
  });

  // Every device seeds from its cache and answers on its own schedule, so the
  // first snapshot delivered is often only one of them. Resolving on it would
  // drop a bookmark on the inbox while the rows that answer it are one poll
  // away — or open the wrong machine's copy of the same project id.
  it("waits until every live device has answered rather than resolving on the first one", async () => {
    openSession("dev-1");
    openSession("dev-2");
    const theirs = checkoutRow("dev-2", "their-main");
    const mine = checkoutRow("dev-1", "main");
    feedSnapshot = merge({ "dev-2": { items: [theirs] } });
    App.route = { name: "resolve", kind: "worktree", projectId: "p1", id: "wt-1", tab: "changes" };

    renderResolving();
    expect(App.route.name).toBe("resolve"); // this device has not said anything yet

    deliverFeed(merge({ "dev-2": { items: [theirs] }, "dev-1": { items: [mine] } }));

    expect(App.route).toEqual({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" });
  });

  // A device whose session is up but whose board.list keeps failing writes no
  // view at all — taskFeed swallows the error — so a link that waited for every
  // live device would spin for as long as that bridge stays sick. The wait is
  // bounded: what the other machines answered is the best answer there is.
  it("lands on what it has when a live device never answers", () => {
    vi.useFakeTimers();
    openSession("dev-1");
    openSession("dev-2"); // its board.list never lands
    feedSnapshot = merge({ "dev-1": { items: [checkoutRow("dev-1", "main")] } });
    App.route = { name: "resolve", kind: "worktree", projectId: "p1", id: "wt-1", tab: "changes" };

    renderResolving();
    expect(App.route.name).toBe("resolve");

    vi.advanceTimersByTime(10_000);

    expect(App.route).toEqual({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" });
    vi.useRealTimers();
  });

  // A boot paint is nobody answering — but when no device is in a position to
  // answer, it is everything there is, and a link has to land somewhere rather
  // than spin.
  it("resolves from the boot paint when no device is live", async () => {
    App.devices = [{ id: "dev-1", name: "This device", status: "offline" }];
    openSession("dev-1");
    contexts.setContextOffline("dev-1");
    const mine = checkoutRow("dev-1", "main");
    feedSnapshot = merge({ "dev-1": { items: [mine], cached: true } });
    App.route = { name: "resolve", kind: "worktree", projectId: "p1", id: "wt-1", tab: "changes" };

    renderResolving();

    expect(App.route).toEqual({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" });
  });
});

// A link somebody sent before routes carried a device, or one typed by hand:
// the project id is one every machine mints, so the account is searched and the
// home device wins.
describe("a work link with no device in it", () => {
  it("lands on the home device's copy of the project", async () => {
    openSession("dev-1");
    openSession("dev-2");
    const theirs = { kind: "branch", project_id: "p1", branch: "main", deviceId: "dev-2" };
    const mine = { kind: "branch", project_id: "p1", branch: "main", deviceId: "dev-1" };
    feedSnapshot = merge({ "dev-2": { items: [theirs] }, "dev-1": { items: [mine] } });
    App.route = routeFromHash("#/project/p1/branch/main/files?path=a%2Fb");
    expect(App.route.kind).toBe("project");

    renderResolving();

    expect(App.route).toEqual({
      name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "files", file: "a/b",
    });
  });

  it("finds a plain folder that has no work row on any device", async () => {
    openSession("dev-2");
    feedSnapshot = merge({ "dev-2": { items: [], projects: [{ id: "p1", name: "notes", deviceId: "dev-2" }] } });
    App.route = routeFromHash("#/project/p1/branch/main/files");

    renderResolving();

    expect(App.route).toEqual({ name: "branch", deviceId: "dev-2", projectId: "p1", branch: "main", tab: "files" });
  });
});
