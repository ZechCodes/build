// @vitest-environment jsdom
// Where a pre-redesign URL waits. Those URLs name one machine's checkout — the
// home device's, until a route can name its own — so the rows they are looked
// up in are that machine's and not every device's at once. Every machine mints
// a `proj-1`, so `#/p/proj-1/changes` says nothing about which one.

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
  refreshFeed: async () => {},
  primaryRunIdFor: () => null,
  dropFeedDevice: () => {},
}));

const { App } = await import("../src/app.js");
const { renderResolving } = await import("../src/views/resolving.js");
const contexts = await import("../src/core/deviceContexts.js");

const primaryRow = (deviceId, branch) => ({
  kind: "branch",
  project_id: "p1",
  branch,
  primary: true,
  worktree_id: `wt-${deviceId}`,
  deviceId,
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
    projects: [],
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
  contexts.setHomeContext(
    contexts.adoptDeviceSession({
      deviceId: "dev-1",
      call: async () => ({}),
      close: () => {},
      peer: () => {},
      onCarrier: () => {},
    }),
  );
});

afterEach(() => {
  contexts.resetDeviceContexts();
  App.devices = [];
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
});

describe("a legacy URL on an account with more than one device", () => {
  it("opens the home device's checkout, not the one that sorts first", async () => {
    // The desktop's own `p1` sorts first in the merge.
    const theirs = primaryRow("dev-2", "their-main");
    const mine = primaryRow("dev-1", "main");
    feedSnapshot = {
      items: [theirs, mine],
      projects: [],
      devices: { "dev-2": { items: [theirs], projects: [] }, "dev-1": { items: [mine], projects: [] } },
    };
    App.route = { name: "resolve", kind: "primary", projectId: "p1", tab: "changes" };

    renderResolving();

    expect(App.route).toEqual({ name: "branch", projectId: "p1", branch: "main", tab: "changes" });
  });

  // Every device seeds from its cache and answers on its own schedule, so the
  // first snapshot delivered is often somebody else's. Resolving on it would
  // drop a bookmark on the inbox while the rows that answer it are one poll
  // away.
  it("waits for the home device to answer rather than resolving on another one", async () => {
    const theirs = primaryRow("dev-2", "their-main");
    const mine = primaryRow("dev-1", "main");
    feedSnapshot = merge({ "dev-2": { items: [theirs] } });
    App.route = { name: "resolve", kind: "primary", projectId: "p1", tab: "changes" };

    renderResolving();
    expect(App.route.name).toBe("resolve"); // the desktop says nothing about this link

    deliverFeed(merge({ "dev-2": { items: [theirs] }, "dev-1": { items: [mine] } }));

    expect(App.route).toEqual({ name: "branch", projectId: "p1", branch: "main", tab: "changes" });
  });

  // The boot paint is not the home device answering — but when that device
  // cannot answer at all, it is everything there is, and a link has to land
  // somewhere rather than spin.
  it("resolves from the boot paint when the home device is offline", async () => {
    const mine = primaryRow("dev-1", "main");
    contexts.setContextOffline("dev-1");
    feedSnapshot = merge({ "dev-1": { items: [mine], cached: true } });
    App.route = { name: "resolve", kind: "primary", projectId: "p1", tab: "changes" };

    renderResolving();

    expect(App.route).toEqual({ name: "branch", projectId: "p1", branch: "main", tab: "changes" });
  });
});
