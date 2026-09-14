// @vitest-environment jsdom
// One context per paired device: the object every surface reads instead of the
// four singletons. Adoption is create-or-retarget, retirement is final, and the
// order the registry answers in is the order the device list is in.

import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "../src/app.js";
import {
  adoptBridgeSelection,
  adoptDeviceSession,
  canAnswer,
  contextFor,
  homeContext,
  deviceFeedView,
  knownContexts,
  liveContexts,
  onDeviceStateChanged,
  resetDeviceContexts,
  retireDeviceContext,
  routeContext,
  setContextOffline,
} from "../src/core/deviceContexts.js";
import { bridgeBehindMark, deviceOfflineMark } from "../src/core/text.js";
import { callerReaches, fakeSession } from "./deviceSessionFixture.js";

const deviceIdsOf = (contexts) => contexts.map((context) => context.deviceId);

/** The account's device list as the app reads it: an id and whether that
 *  device can answer. */
const online = (id) => ({ id, status: "online" });
const offline = (id) => ({ id, status: "offline" });

/** Name a device home the way the running app does: the account lists it
 *  online and the user's pick names it. */
function adoptHome(deviceId) {
  App.devices = [online(deviceId)];
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

  it("keeps a re-adopted device's controllers and drafts, and sends on the new transport", async () => {
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
    // The repository's caller is the device's, not the session's spy: it refuses
    // when the machine cannot answer, so ask through it and see where it lands.
    expect(await callerReaches(chatRepository.currentCall(), reconnected.call)).toBe(true);
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

  // One predicate, not two: "the contexts that can answer" and "can this
  // machine be asked anything" were written separately and could drift.
  it("answers with exactly the contexts that canAnswer", () => {
    App.devices = [online("dev-a"), online("dev-b"), online("dev-c")];
    adoptDeviceSession(fakeSession("dev-a"));
    adoptDeviceSession(fakeSession("dev-b"));
    adoptDeviceSession(fakeSession("dev-c"));
    setContextOffline("dev-b");
    retireDeviceContext("dev-c");

    expect(deviceIdsOf(liveContexts())).toEqual(knownContexts().filter(canAnswer).map((c) => c.deviceId));
    expect(deviceIdsOf(liveContexts())).toEqual(["dev-a"]);
  });

  // Whether a machine can answer is not news the feed carries, and a device
  // the account has let go of can answer nothing. The gate counting the live
  // machines, the strip over an open surface and the rail's greying all hear it
  // the one way — so retirement says so, exactly as going offline does.
  it("tells the surfaces a retired device can no longer answer", () => {
    adoptDeviceSession(fakeSession("dev-a"));
    const told = vi.fn();
    const stopListening = onDeviceStateChanged(told);

    retireDeviceContext("dev-a");

    expect(told).toHaveBeenCalledTimes(1);
    stopListening();
  });

  // A controller is addressed against its device's scope: retiring the device
  // takes the address away, so a pane still holding one cannot write a draft or
  // a message into a machine the app has let go of.
  it("shuts a retired device's controllers", () => {
    const context = adoptDeviceSession(fakeSession("dev-a"));
    const controller = context.chatRepository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "thread-1",
    });

    retireDeviceContext("dev-a");

    expect(() => controller.writeDraft({ body: "too late" })).toThrow(/no longer active/);
    expect(controller.threadState.active()).toBe(false);
  });

  // The transport a submission was accepted on is the one it finishes on: a
  // reconnect landing mid-flight must not make the new socket answer for a send
  // the old one already took, or the message goes out twice.
  it("a send accepted before a reconnect goes out on the transport that accepted it", async () => {
    let release;
    const first = fakeSession("dev-a");
    first.call = vi.fn(() => new Promise((resolve) => { release = resolve; }));
    const reconnected = fakeSession("dev-a");
    const controller = adoptDeviceSession(first).chatRepository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "thread-1",
    });
    controller.writeDraft({ body: "captured" });
    const submission = controller.captureSubmission();
    const pending = controller.post(submission);

    adoptDeviceSession(reconnected);
    release({ operation_id: submission.operationId });
    await pending;

    expect(first.call).toHaveBeenCalledTimes(1);
    expect(reconnected.call).not.toHaveBeenCalled();
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

  // Every surface that stands a frame up over a machine — a branch, an issue,
  // a sheet the toolbar opens — asks the same question before it paints, so the
  // question is asked in one place and worded once.
  it("says a context can answer only while it is here and nothing has marked it offline", () => {
    const context = adoptDeviceSession(fakeSession("dev-a"));

    expect(canAnswer(context)).toBe(true);

    setContextOffline("dev-a", { offline: true });

    expect(canAnswer(context)).toBe(false);

    setContextOffline("dev-a", { offline: false });

    expect(canAnswer(context)).toBe(true);

    retireDeviceContext("dev-a");

    expect(canAnswer(context)).toBe(false);
  });

  // A link can name a machine this client has never opened: there is no context
  // to ask, which is the same answer as one that cannot answer.
  it("says an absent context cannot answer", () => {
    expect(canAnswer(contextFor("dev-nobody"))).toBe(false);
    expect(canAnswer(null)).toBe(false);
  });

  // The caller every surface holds is the one seam they all share: a mount asks
  // canAnswer before it paints, but a poll already running, a watcher already
  // subscribed and the cache syncer's background tier ask nothing — they just
  // call. A machine that cannot answer must refuse at the caller, or a surface
  // mounted before the greeting settled goes on asking a bridge whose answers
  // this tab cannot read, and paints whatever comes back.
  it("refuses a call to a machine that cannot answer, and says which way it cannot", async () => {
    const session = fakeSession("dev-a");
    const context = adoptDeviceSession(session);

    expect(await callerReaches(context.rpc, session.call)).toBe(true);

    adoptBridgeSelection(context, { version: "0.9.0", unsupported: "bridge" }, null);
    const asked = session.call.mock.calls.length;

    await expect(context.rpc("board.list")).rejects.toThrow(bridgeBehindMark);
    expect(session.call.mock.calls.length).toBe(asked);

    adoptBridgeSelection(context, { version: "1.1.0", unsupported: null }, {});
    setContextOffline("dev-a", { offline: true });

    await expect(context.rpc("board.list")).rejects.toThrow(deviceOfflineMark);
    expect(session.call.mock.calls.length).toBe(asked);
  });

  // The conversations are the other caller a mounted surface holds: the rail
  // reads and posts through the repository, which is bound to the session that
  // adopted it so a post finishes where it was accepted. That binding must not
  // be a way round the refusal.
  it("refuses the conversations of a machine that cannot answer", async () => {
    const session = fakeSession("dev-a");
    const context = adoptDeviceSession(session);
    adoptBridgeSelection(context, { version: "0.9.0", unsupported: "bridge" }, null);

    await expect(context.chatRepository.currentCall()("branch.get", {})).rejects.toThrow(bridgeBehindMark);
    expect(session.call).not.toHaveBeenCalled();
  });

  // A reconnect re-greets, and the bridge that answers may be the one that can
  // be read: the caller the mounted surface is still holding has to come back
  // with the device.
  it("lets a machine be asked again once its next greeting can be read", async () => {
    const session = fakeSession("dev-a");
    const context = adoptDeviceSession(session);
    adoptBridgeSelection(context, { version: "0.9.0", unsupported: "bridge" }, null);

    const resumed = fakeSession("dev-a");
    adoptDeviceSession(resumed);
    adoptBridgeSelection(context, { version: "1.1.0", unsupported: null }, {});

    expect(await callerReaches(context.rpc, resumed.call)).toBe(true);
  });

  it("calls home the picked device while it is online, else the first online device, else nothing", () => {
    App.devices = [online("dev-a"), online("dev-b")];
    const first = adoptDeviceSession(fakeSession("dev-a"));
    const second = adoptDeviceSession(fakeSession("dev-b"));

    App.selectedDeviceId = "dev-b";

    expect(homeContext()).toBe(second);

    App.selectedDeviceId = null;

    expect(homeContext()).toBe(first);

    App.devices = [offline("dev-a"), online("dev-b")];

    expect(homeContext()).toBe(second);

    App.devices = [offline("dev-a"), offline("dev-b")];

    expect(homeContext()).toBe(null);
  });

  // Boot opens every online device at once, so the device the account lists
  // first can still be handshaking when the second one lands. Home is that
  // device's, and nobody else's, from the moment it answers.
  it("names no home while the device it would name has no context yet", () => {
    App.devices = [online("dev-a"), online("dev-b")];
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
    App.devices = [online("dev-a"), online("dev-b")];
    App.selectedDeviceId = "dev-a";
    const first = adoptDeviceSession(fakeSession("dev-a"));
    const second = adoptDeviceSession(fakeSession("dev-b"));

    expect(routeContext({ name: "branch", deviceId: "dev-b", projectId: "p1" })).toBe(second);
    expect(routeContext({ name: "branch", deviceId: "dev-a", projectId: "p1" })).toBe(first);
    expect(routeContext({ name: "inbox" })).toBe(null);
    expect(routeContext({ name: "branch", deviceId: "dev-z" })).toBe(null);
    expect(routeContext(null)).toBe(null);
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

// The harnesses a machine offers are that machine's: two bridges on one account
// can be built from different releases and offer different agents. So the
// answer is held on the context that asked for it, and it is asked for once.
describe("one device's model catalog", () => {
  /** A bridge that answers models.list with `answer`, and nothing else. */
  const bridgeOffering = (deviceId, answer) => ({
    ...fakeSession(deviceId),
    call: vi.fn(async (method) => (method === "models.list" ? answer : {})),
  });

  const offering = (id) => ({ providers: [{ id, label: id, models: [], efforts: [] }] });
  const listsCalled = (context) => context.session.call.mock.calls.filter(([method]) => method === "models.list").length;

  it("answers one models.list per device and hands the same catalog back after", async () => {
    const first = adoptDeviceSession(bridgeOffering("dev-a", offering("claude")));
    const second = adoptDeviceSession(bridgeOffering("dev-b", offering("codex")));

    const held = await first.modelCatalog();

    expect(held.providers[0].id).toBe("claude");
    expect((await second.modelCatalog()).providers[0].id).toBe("codex");
    expect(await first.modelCatalog()).toBe(held);
    expect(listsCalled(first)).toBe(1);
    expect(listsCalled(second)).toBe(1);
  });

  // An older bridge has no models.list at all. Its catalog is the empty one, so
  // every selector offers the harness's own default — which is exactly what
  // that bridge supports — rather than the surface failing to paint.
  it("answers an empty catalog from a bridge without the RPC", async () => {
    const session = fakeSession("dev-a");
    session.call = vi.fn(async () => {
      throw new Error("unknown method: models.list");
    });

    const catalog = await adoptDeviceSession(session).modelCatalog();

    expect(catalog.providers).toHaveLength(1);
    expect(catalog.providers[0].models).toEqual([]);
    expect(catalog.providers[0].efforts).toEqual([]);
  });

  it("reads models.list again on refreshModelCatalog and replaces what it held", async () => {
    let offered = offering("claude");
    const session = fakeSession("dev-a");
    session.call = vi.fn(async () => offered);
    const context = adoptDeviceSession(session);

    expect((await context.modelCatalog()).providers[0].id).toBe("claude");

    offered = offering("codex");

    expect((await context.refreshModelCatalog()).providers[0].id).toBe("codex");
    expect((await context.modelCatalog()).providers[0].id).toBe("codex");
    expect(listsCalled(context)).toBe(2);
  });

  // A read can outlive the device it was asked of. Its answer still belongs to
  // whoever asked, but it must not become the catalog of anything afterwards.
  it("populates nothing from an answer that lands after the device was retired", async () => {
    let release;
    const session = fakeSession("dev-a");
    session.call = vi.fn(() => new Promise((resolve) => { release = resolve; }));
    const retiring = adoptDeviceSession(session);
    const late = retiring.modelCatalog();

    retireDeviceContext("dev-a");
    release(offering("retired-device"));

    expect((await late).providers[0].id).toBe("retired-device");

    const readopted = adoptDeviceSession(bridgeOffering("dev-a", offering("landed-again")));

    expect(readopted).not.toBe(retiring);
    expect((await readopted.modelCatalog()).providers[0].id).toBe("landed-again");
  });
});
