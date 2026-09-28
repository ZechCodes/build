// @vitest-environment jsdom
// The subscriptions this client holds: the bridge flushes what moved, and the
// surfaces standing for those entities hear it. Nothing here runs on a timer,
// and a registration that asks for one is refused — an old bridge that never
// arms is a machine this client hears nothing from, which is what the ordered
// pass on every greeting is for.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let armChangeEvents,
  bridgeApiVersion,
  changeEventsArmed,
  disarmChangeEvents,
  dispatchChangeEvent,
  greetBridge,
  refetchEverything,
  resetChangeEvents,
  watchChanges;

const setHidden = (hidden) =>
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });

const becomeVisible = () => {
  setHidden(false);
  document.dispatchEvent(new Event("visibilitychange"));
};

/** One flush off a machine: the bodies of what moved, addressed by entity.
 *  The board's own line rides under the id the bridge gives it. */
const flush = (items, deviceId) => dispatchChangeEvent({ type: "changes", items }, deviceId);

/** The board moved on this machine. */
const boardMoved = (deviceId) => flush([{ entity_id: "board", state: {} }], deviceId);

/** One entity moved on this machine, carrying the body named. */
const entityMoved = (entityId, deviceId, body = { state: {} }) => flush([{ entity_id: entityId, ...body }], deviceId);

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  setHidden(false);
  ({
    armChangeEvents,
    bridgeApiVersion,
    changeEventsArmed,
    disarmChangeEvents,
    dispatchChangeEvent,
    greetBridge,
    refetchEverything,
    resetChangeEvents,
    watchChanges,
  } = await import("../src/core/changeEvents.js"));
});

afterEach(() => {
  resetChangeEvents();
  vi.useRealTimers();
});

describe("arming", () => {
  it("arms on the bridge's push_events flag", () => {
    expect(armChangeEvents({ push_events: true, events: ["board.changed"] }, "dev-a")).toBe(true);
    expect(changeEventsArmed("dev-a")).toBe(true);
  });

  it("stays unarmed for a bridge that never answered the greeting", () => {
    expect(armChangeEvents(null, "dev-a")).toBe(false);
    expect(changeEventsArmed("dev-a")).toBe(false);
  });

  it("stays unarmed for a greeting without the flag", () => {
    expect(armChangeEvents({ pong: true }, "dev-a")).toBe(false);
    expect(armChangeEvents({ push_events: false }, "dev-a")).toBe(false);
    expect(changeEventsArmed("dev-a")).toBe(false);
  });
});

// A surface here has no poll at all: it subscribes, it hears, it paints. The
// registry used to run one per surface, so the one way to register without a
// poll was to invent a cadence nobody wanted.
describe("the cadence there is not", () => {
  it("starts no timer, for any registration", () => {
    const refresh = vi.fn();
    watchChanges({ refresh, entity: "run-1", kinds: ["git"] });
    watchChanges({ refresh, kinds: ["state"] });
    vi.advanceTimersByTime(60 * 60 * 1000);
    expect(refresh).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  // Honouring a cadence quietly would put a poll back without anybody reading
  // a line of the module, so a registration naming one is refused outright.
  it("takes a registration that names no cadence, and no other shape", () => {
    const refresh = vi.fn();
    expect(() => watchChanges({ refresh, intervalMs: 1600 })).toThrow(/does not poll/);
    expect(() => watchChanges({ refresh, keepPolling: true })).toThrow(/does not poll/);
    expect(() => watchChanges({ refresh, catchUpOnVisible: false })).toThrow(/does not poll/);
    expect(() => watchChanges({ refresh, entity: "run-1", kinds: ["git"] })).not.toThrow();
  });

  it("delivers a push to a watcher, and stops once it is disposed", () => {
    const refresh = vi.fn();
    armChangeEvents({ push_events: true }, "dev-a");
    const watcher = watchChanges({ refresh, entity: "run-1", deviceId: "dev-a", kinds: ["git"] });
    entityMoved("run-1", "dev-a", { git: {} });
    expect(refresh).toHaveBeenCalledTimes(1);

    watcher.dispose();
    entityMoved("run-1", "dev-a", { git: {} });
    expect(refresh).toHaveBeenCalledTimes(1);
  });
});

describe("the board's own item", () => {
  it("refetches every board-scoped surface exactly once", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const feed = vi.fn();
    watchChanges({ refresh: feed });
    boardMoved("dev-a");
    expect(feed).toHaveBeenCalledTimes(1);
  });

  it("leaves entity surfaces alone — their own event says when they moved", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const detail = vi.fn();
    watchChanges({ refresh: detail, entity: "run-7" });
    boardMoved("dev-a");
    expect(detail).not.toHaveBeenCalled();
  });
});

// An event kind this client has nothing registered for is not news: it wakes
// nobody, and says so, whatever the wire calls it.
describe("an event this client does not act on", () => {
  it("wakes nobody for an unknown kind, or an entity event that names no id", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const feed = vi.fn();
    const detail = vi.fn();
    watchChanges({ refresh: feed });
    watchChanges({ refresh: detail, entity: "run-7" });

    expect(dispatchChangeEvent({ type: "session.hello" }, "dev-a")).toBe(false);
    expect(dispatchChangeEvent({ type: "constructor" }, "dev-a")).toBe(false);
    // The bridge still sends the legacy hints. This client paints from the
    // cache and acts on bodies, so a hint is not news it can do anything with.
    expect(dispatchChangeEvent({ type: "board.changed" }, "dev-a")).toBe(false);
    expect(dispatchChangeEvent({ type: "entity.changed", id: "run-7" }, "dev-a")).toBe(false);
    expect(flush([], "dev-a")).toBe(false);
    expect(feed).not.toHaveBeenCalled();
    expect(detail).not.toHaveBeenCalled();
  });
});

describe("an entity's item", () => {
  it("refetches only the surfaces showing that entity", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const shown = vi.fn();
    const other = vi.fn();
    const feed = vi.fn();
    watchChanges({ refresh: shown, entity: "run-7" });
    watchChanges({ refresh: other, entity: "run-9" });
    watchChanges({ refresh: feed });
    entityMoved("run-7", "dev-a");
    expect(shown).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
    expect(feed).not.toHaveBeenCalled();
  });

  it("reads the entity at delivery, so a surface that moved is asked about now", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    let showing = "run-7";
    const refresh = vi.fn();
    watchChanges({ refresh, entity: () => showing });
    showing = "run-9";
    entityMoved("run-7", "dev-a");
    expect(refresh).not.toHaveBeenCalled();
    entityMoved("run-9", "dev-a");
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("matches any of the ids a surface stands for", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, entity: () => ["run-7", "wt-3"] });
    entityMoved("wt-3", "dev-a");
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("ignores an entity surface that does not know its id yet", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, entity: () => null });
    entityMoved("run-7", "dev-a");
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("an unarmed client", () => {
  it("ignores change events entirely", () => {
    const feed = vi.fn();
    const detail = vi.fn();
    watchChanges({ refresh: feed });
    watchChanges({ refresh: detail, entity: "run-7" });
    boardMoved("dev-a");
    entityMoved("run-7", "dev-a");
    expect(feed).not.toHaveBeenCalled();
    expect(detail).not.toHaveBeenCalled();
  });
});

describe("a hidden tab", () => {
  it("skips the refetch an event asks for, exactly as it skips a poll", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh });
    setHidden(true);
    boardMoved("dev-a");
    expect(refresh).not.toHaveBeenCalled();
  });

  it("catches up on the events it missed the moment it comes back", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh });
    setHidden(true);
    boardMoved("dev-a");
    boardMoved("dev-a");
    becomeVisible();
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("has nothing to catch up on when no event arrived while it was away", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh });
    setHidden(true);
    becomeVisible();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("still delivers to a surface whose poll does not pause while hidden", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, entity: "iss-1", pausesWhileHidden: false });
    setHidden(true);
    entityMoved("iss-1", "dev-a");
    expect(refresh).toHaveBeenCalledTimes(1);
  });
});

describe("the greeting", () => {
  it("arms event mode when the bridge answers the flag", async () => {
    const call = vi.fn(async () => ({ push_events: true, events: ["board.changed"] }));
    await greetBridge(call, { deviceId: "dev-a" });
    expect(call).toHaveBeenCalledWith("session.hello", expect.objectContaining({ client: expect.any(Object) }));
    expect(changeEventsArmed("dev-a")).toBe(true);
  });

  it("declares who is calling: the SPA, its build, and the API range it speaks", async () => {
    const call = vi.fn(async () => ({ push_events: true }));
    await greetBridge(call);
    expect(call).toHaveBeenCalledWith("session.hello", {
      client: { name: "spa", version: expect.any(String), api_range: ">=2.0.0 <3.0.0" },
      changes: "subscriptions",
    });
    expect(call.mock.calls[0][1].client.version).not.toBe("");
  });

  it("remembers the bridge's api_version, and reads 0.0.0 from a bridge that reports none", async () => {
    expect(bridgeApiVersion()).toBe("0.0.0");
    await greetBridge(async () => ({ push_events: true, api_version: "2.0.0" }));
    expect(bridgeApiVersion()).toBe("2.0.0");
    await greetBridge(async () => ({ push_events: true }));
    expect(bridgeApiVersion()).toBe("0.0.0");
    await greetBridge(async () => ({ push_events: true, api_version: "2.0.0" }));
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    });
    expect(bridgeApiVersion()).toBe("0.0.0");
  });

  it("still hands the whole greeting, api_version included, to onGreeting", async () => {
    const accepted = vi.fn();
    const greeting = { push_events: true, api_version: "2.0.0" };
    await greetBridge(async () => greeting, { onGreeting: accepted });
    expect(accepted).toHaveBeenCalledWith(greeting);
  });

  it("leaves a bridge that refuses the greeting unarmed, hearing nothing", async () => {
    const call = vi.fn(async () => {
      throw new Error("unknown method: session.hello");
    });
    const refresh = vi.fn();
    watchChanges({ refresh, entity: "run-7" });
    await greetBridge(call, { deviceId: "dev-a" });

    expect(changeEventsArmed("dev-a")).toBe(false);
    expect(entityMoved("run-7", "dev-a")).toBe(false);
    // The greeting itself still reads everything once: the gap behind a
    // session that has just been greeted announced nothing.
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("requires an actual transport acknowledgement for a strict initial greeting", async () => {
    const dropped = new Error("your device went offline");
    await expect(greetBridge(async () => { throw dropped; }, { deviceId: "dev-a", strict: true })).rejects.toBe(dropped);
    await expect(greetBridge(async () => { throw new Error("unknown method: session.hello"); }, { strict: true })).resolves.toBe(false);
  });

  it("refetches every surface, whichever mode it lands in", async () => {
    const armedRefresh = vi.fn();
    watchChanges({ refresh: armedRefresh });
    await greetBridge(async () => ({ push_events: true }), { deviceId: "dev-a" });
    expect(armedRefresh).toHaveBeenCalledTimes(1);

    resetChangeEvents();
    const pollingRefresh = vi.fn();
    watchChanges({ refresh: pollingRefresh });
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    }, { deviceId: "dev-a" });
    expect(pollingRefresh).toHaveBeenCalledTimes(1);
  });

  it("disarms when a reconnect lands on a bridge that cannot push", async () => {
    await greetBridge(async () => ({ push_events: true }), { deviceId: "dev-a" });
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    }, { deviceId: "dev-a" });
    expect(changeEventsArmed("dev-a")).toBe(false);
  });

  it("ignores a late greeting after its session stops owning the application", async () => {
    let resolveGreeting;
    let current = true;
    const refreshed = vi.fn();
    const accepted = vi.fn();
    watchChanges({ refresh: refreshed });
    const pending = greetBridge(
      () => new Promise((resolve) => { resolveGreeting = resolve; }),
      { deviceId: "dev-a", isCurrent: () => current, onGreeting: accepted },
    );
    current = false;
    resolveGreeting({ push_events: true, thread_post_operations: { version: 1, status_method: "operation.get" } });
    await pending;

    expect(changeEventsArmed("dev-a")).toBe(false);
    expect(refreshed).not.toHaveBeenCalled();
    expect(accepted).not.toHaveBeenCalled();
  });
});

describe("a reconnect", () => {
  it("refetches every surface once — the gap announced nothing", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const feed = vi.fn();
    const detail = vi.fn();
    watchChanges({ refresh: feed });
    watchChanges({ refresh: detail, entity: "run-7" });
    refetchEverything();
    expect(feed).toHaveBeenCalledTimes(1);
    expect(detail).toHaveBeenCalledTimes(1);
  });

  it("refetches an unarmed device's surfaces too — that gap loses just as much", () => {
    const feed = vi.fn();
    watchChanges({ refresh: feed });
    refetchEverything();
    expect(feed).toHaveBeenCalledTimes(1);
  });
});

// ---- more than one device --------------------------------------------------
// Every device pushes its own events down its own session. A watcher that named
// a device hears only that device; the merged inbox names none and hears them
// all, which is what makes one list out of several machines.
describe("several devices", () => {
  it("refreshes B's feed watcher and the merged inbox watcher on B's board item, not A's", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents({ push_events: true }, "dev-b");
    const feedA = vi.fn();
    const feedB = vi.fn();
    const inbox = vi.fn();
    watchChanges({ refresh: feedA, deviceId: "dev-a" });
    watchChanges({ refresh: feedB, deviceId: "dev-b" });
    watchChanges({ refresh: inbox });

    boardMoved("dev-b");

    expect(feedB).toHaveBeenCalledTimes(1);
    expect(inbox).toHaveBeenCalledTimes(1);
    expect(feedA).not.toHaveBeenCalled();
  });

  it("keeps armed state per device", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents(null, "dev-b");
    expect(changeEventsArmed("dev-a")).toBe(true);
    expect(changeEventsArmed("dev-b")).toBe(false);

    const feedB = vi.fn();
    watchChanges({ refresh: feedB, deviceId: "dev-b" });
    expect(boardMoved("dev-b")).toBe(false);
    expect(feedB).not.toHaveBeenCalled();
  });

  // "Is anything pushing?" is only true of the account when every machine on
  // it pushes: one machine that does not is a machine nothing would announce.
  it("calls the account armed only when every known device is", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents(null, "dev-b");
    expect(changeEventsArmed()).toBe(false);
    expect(changeEventsArmed("dev-a")).toBe(true);

    armChangeEvents({ push_events: true }, "dev-b");
    expect(changeEventsArmed()).toBe(true);
  });

  it("forgets a device's armed state when it is disarmed", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents({ push_events: true }, "dev-b");
    disarmChangeEvents("dev-b");

    expect(changeEventsArmed("dev-b")).toBe(false);
    expect(changeEventsArmed("dev-a")).toBe(true);
    // The retired device is forgotten, not counted as one that pushes nothing.
    expect(changeEventsArmed()).toBe(true);
    expect(boardMoved("dev-b")).toBe(false);
  });

  it("arms the device greetBridge greeted and refetches that device's and the any-device watchers", async () => {
    const feedA = vi.fn();
    const feedB = vi.fn();
    const inbox = vi.fn();
    watchChanges({ refresh: feedA, deviceId: "dev-a" });
    watchChanges({ refresh: feedB, deviceId: "dev-b" });
    watchChanges({ refresh: inbox });

    await greetBridge(async () => ({ push_events: true }), { deviceId: "dev-b" });

    expect(changeEventsArmed("dev-b")).toBe(true);
    expect(changeEventsArmed("dev-a")).toBe(false);
    expect(feedB).toHaveBeenCalledTimes(1);
    expect(inbox).toHaveBeenCalledTimes(1);
    expect(feedA).not.toHaveBeenCalled();
  });

  it("refetches every watcher when no device is named — what a reconnect asks for", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents({ push_events: true }, "dev-b");
    const feedA = vi.fn();
    const feedB = vi.fn();
    watchChanges({ refresh: feedA, deviceId: "dev-a" });
    watchChanges({ refresh: feedB, deviceId: "dev-b" });

    refetchEverything();

    expect(feedA).toHaveBeenCalledTimes(1);
    expect(feedB).toHaveBeenCalledTimes(1);
  });
});
