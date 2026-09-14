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
  knownContexts,
  liveContexts,
  resetDeviceContexts,
  retireDeviceContext,
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

beforeEach(() => {
  resetDeviceContexts();
  App.devices = [];
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

  it("calls home the context the aliases were last pointed at", () => {
    const first = adoptDeviceSession(fakeSession("dev-a"));
    const second = adoptDeviceSession(fakeSession("dev-b"));

    expect(homeContext()).toBe(null);

    pointAliasesAt(first);

    expect(homeContext()).toBe(first);
    expect(App.call).toBe(first.call);
    expect(App.cacheScope).toBe(first.cacheScope);
    expect(App.chatRepository).toBe(first.chatRepository);

    pointAliasesAt(second);

    expect(homeContext()).toBe(second);
    expect(App.session).toBe(second.session);

    retireDeviceContext("dev-b");

    expect(homeContext()).toBe(null);
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
