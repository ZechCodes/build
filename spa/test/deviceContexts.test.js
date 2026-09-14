// @vitest-environment jsdom
// One context per paired device: the object every surface reads instead of the
// four singletons. Adoption is create-or-retarget, retirement is final, and the
// order the registry answers in is the order the device list is in.

import { beforeEach, describe, expect, it, vi } from "vitest";

import { App, pointAliasesAt } from "../src/app.js";
import { currentCacheScope } from "../src/core/cacheScope.js";
import {
  adoptDeviceSession,
  contextFor,
  homeContext,
  deviceFeedView,
  homeProjectKey,
  knownContexts,
  liveContexts,
  resetDeviceContexts,
  retireDeviceContext,
  routeContext,
  setContextOffline,
} from "../src/core/deviceContexts.js";

const fakeSession = (deviceId) => ({
  deviceId,
  call: vi.fn(async () => ({})),
  close: vi.fn(),
  peer: vi.fn(),
  onCarrier: vi.fn(),
  onPush: vi.fn(),
});

const deviceIdsOf = (contexts) => contexts.map((context) => context.deviceId);

const listed = (...devices) => devices.map(([id, status]) => ({ id, status }));

/** Name a device home the way the running app does: the account lists it
 *  online and the user's pick names it. */
function adoptHome(deviceId) {
  App.devices = listed([deviceId, "online"]);
  App.selectedDeviceId = deviceId;
  return adoptDeviceSession(fakeSession(deviceId));
}

beforeEach(() => {
  resetDeviceContexts();
  App.devices = [];
  App.selectedDeviceId = null;
});

describe("the device context registry", () => {
  it("gives every adopted session its own context", () => {
    const first = adoptDeviceSession(fakeSession("dev-a"));
    const second = adoptDeviceSession(fakeSession("dev-b"));

    expect(first).not.toBe(second);
    expect(contextFor("dev-a")).toBe(first);
    expect(contextFor("dev-b")).toBe(second);
    expect(contextFor("dev-z")).toBe(null);
    expect(first.cacheScope).not.toBe(second.cacheScope);
    expect(first.chatRepository).not.toBe(second.chatRepository);
    expect(first.cacheScope.deviceId).toBe("dev-a");
    expect(first.active()).toBe(true);
  });

  it("keeps a re-adopted device's scope and repository, and takes its new transport", () => {
    const context = adoptDeviceSession(fakeSession("dev-a"));
    const { cacheScope, chatRepository } = context;
    const controller = chatRepository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "thread-1",
    });
    controller.writeDraft({ body: "keep this" });

    const reconnected = fakeSession("dev-a");
    const resumed = adoptDeviceSession(reconnected);

    expect(resumed).toBe(context);
    expect(resumed.cacheScope).toBe(cacheScope);
    expect(resumed.chatRepository).toBe(chatRepository);
    expect(resumed.call).toBe(reconnected.call);
    expect(resumed.session).toBe(reconnected);
    expect(chatRepository.currentCall()).toBe(reconnected.call);
    expect(controller.readDraft().body).toBe("keep this");
    expect(resumed.offline).toBe(false);
  });

  it("retires one device without touching another", () => {
    const sessionA = fakeSession("dev-a");
    const retiring = adoptDeviceSession(sessionA);
    const staying = adoptDeviceSession(fakeSession("dev-b"));
    const controller = retiring.chatRepository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "thread-1",
    });

    retireDeviceContext("dev-a");

    expect(contextFor("dev-a")).toBe(null);
    expect(retiring.cacheScope.active()).toBe(false);
    expect(retiring.active()).toBe(false);
    expect(sessionA.close).toHaveBeenCalledTimes(1);
    expect(() => controller.writeDraft({ body: "too late" })).toThrow(/no longer active/);
    expect(contextFor("dev-b")).toBe(staying);
    expect(staying.cacheScope.active()).toBe(true);
    expect(staying.session.close).not.toHaveBeenCalled();
  });

  it("lists the open, unpaused contexts in device-list order, unknown devices last", () => {
    App.devices = [{ id: "dev-b", status: "online" }, { id: "dev-a", status: "online" }];
    adoptDeviceSession(fakeSession("dev-a"));
    adoptDeviceSession(fakeSession("dev-b"));
    adoptDeviceSession(fakeSession("dev-new")); // approved elsewhere; not in the list yet

    expect(deviceIdsOf(liveContexts())).toEqual(["dev-b", "dev-a", "dev-new"]);

    setContextOffline("dev-b", { offline: true, sinceMs: 1_700_000_000_000 });

    expect(deviceIdsOf(liveContexts())).toEqual(["dev-a", "dev-new"]);
    expect(deviceIdsOf(knownContexts())).toEqual(["dev-b", "dev-a", "dev-new"]);
    expect(contextFor("dev-b").offlineSince).toBe(1_700_000_000_000);

    setContextOffline("dev-b", { offline: false });

    expect(deviceIdsOf(liveContexts())).toEqual(["dev-b", "dev-a", "dev-new"]);
    expect(contextFor("dev-b").offlineSince).toBe(null);
  });

  it("calls home the picked device while it is online, else the first online device, else nothing", () => {
    App.devices = listed(["dev-a", "online"], ["dev-b", "online"]);
    const first = adoptDeviceSession(fakeSession("dev-a"));
    const second = adoptDeviceSession(fakeSession("dev-b"));

    App.selectedDeviceId = "dev-b";

    expect(homeContext()).toBe(second);

    App.selectedDeviceId = null;

    expect(homeContext()).toBe(first);

    App.devices = listed(["dev-a", "offline"], ["dev-b", "online"]);

    expect(homeContext()).toBe(second);

    App.devices = listed(["dev-a", "offline"], ["dev-b", "offline"]);

    expect(homeContext()).toBe(null);
  });

  // Boot opens every online device at once, so the device the account lists
  // first can still be handshaking when the second one lands. Home is that
  // device's, and nobody else's, from the moment it answers.
  it("names no home while the device it would name has no context yet", () => {
    App.devices = listed(["dev-a", "online"], ["dev-b", "online"]);
    const second = adoptDeviceSession(fakeSession("dev-b"));

    expect(homeContext()).toBe(null);

    const first = adoptDeviceSession(fakeSession("dev-a"));

    expect(homeContext()).toBe(first);

    retireDeviceContext("dev-a");

    expect(homeContext()).toBe(null);
    expect(contextFor("dev-b")).toBe(second);
  });

  // A work surface is about the machine its link names, whoever is home.
  it("reads the route's device, and nothing for a route without one or a device with no context", () => {
    App.devices = listed(["dev-a", "online"], ["dev-b", "online"]);
    App.selectedDeviceId = "dev-a";
    const first = adoptDeviceSession(fakeSession("dev-a"));
    const second = adoptDeviceSession(fakeSession("dev-b"));

    expect(routeContext({ name: "branch", deviceId: "dev-b", projectId: "p1" })).toBe(second);
    expect(routeContext({ name: "branch", deviceId: "dev-a", projectId: "p1" })).toBe(first);
    expect(routeContext({ name: "inbox" })).toBe(null);
    expect(routeContext({ name: "branch", deviceId: "dev-z" })).toBe(null);
    expect(routeContext(null)).toBe(null);
  });

  it("points the ambient cache-scope alias at the home device too", () => {
    // Surfaces not yet migrated read currentCacheScope() while they mount. It
    // has to name whichever device the aliases were last pointed at, or those
    // reads address no cache at all.
    const first = adoptDeviceSession(fakeSession("dev-a"));
    const second = adoptDeviceSession(fakeSession("dev-b"));

    pointAliasesAt(first);

    expect(currentCacheScope()).toBe(first.cacheScope);

    pointAliasesAt(second);

    expect(currentCacheScope()).toBe(second.cacheScope);

    pointAliasesAt(null);

    expect(currentCacheScope()).toBe(null);
  });
});

// A route names a project by the bare id one bridge minted, and every device
// mints a `proj-1`. A surface that has only a route has to say which device it
// means, and until routes carry one that is the home device.
describe("the home device's name for a project", () => {
  it("pairs the home device with the bare id the route names", () => {
    adoptHome("dev-a");

    expect(homeProjectKey("p1")).toBe("dev-a/p1");
  });

  it("names no project while no device is home, or with no project to name", () => {
    expect(homeProjectKey("p1")).toBe(null);

    adoptHome("dev-a");

    expect(homeProjectKey("")).toBe(null);
    expect(homeProjectKey(undefined)).toBe(null);
  });
});

// The surfaces about where you are — the toolbar, the capture decision page —
// are about one machine, so they read one device's slice of the merged feed
// rather than every device's rows at once. Which device is the home one unless
// something names another: a row's verbs are its own machine's.
describe("one device's view of the feed", () => {
  const mine = { items: [{ id: "a" }], projects: [{ id: "p1" }] };
  const theirs = { items: [{ id: "b" }], projects: [{ id: "p9" }] };
  const merged = { items: [...mine.items, ...theirs.items], devices: { "dev-a": mine, "dev-b": theirs } };

  it("reads the home device's rows and projects out of a merged snapshot", () => {
    adoptHome("dev-a");
    adoptDeviceSession(fakeSession("dev-b"));

    expect(deviceFeedView(merged)).toEqual({ items: mine.items, projects: mine.projects });
  });

  it("reads the device it is given, whoever is home", () => {
    adoptHome("dev-a");
    adoptDeviceSession(fakeSession("dev-b"));

    expect(deviceFeedView(merged, "dev-b")).toEqual({ items: theirs.items, projects: theirs.projects });
  });

  it("answers empty collections while no device is home", () => {
    expect(deviceFeedView(merged)).toEqual({ items: [], projects: [] });
  });
});
