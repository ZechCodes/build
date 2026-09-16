// @vitest-environment jsdom
// Push invalidation, client side: the bridge says what moved, and the surface
// showing it refetches. Everything here is about the two modes being separate —
// an old bridge never arms, and an unarmed client polls exactly as it always
// did.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let armChangeEvents,
  bridgeApiVersion,
  changeEventsArmed,
  disarmChangeEvents,
  dispatchChangeEvent,
  greetBridge,
  pollIntervalMs,
  refetchEverything,
  resetChangeEvents,
  watchChanges,
  SAFETY_POLL_MS;

const setHidden = (hidden) =>
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });

const becomeVisible = () => {
  setHidden(false);
  document.dispatchEvent(new Event("visibilitychange"));
};

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
    pollIntervalMs,
    refetchEverything,
    resetChangeEvents,
    watchChanges,
    SAFETY_POLL_MS,
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

describe("the poll cadence", () => {
  it("keeps polling directories whose external edits have no push watcher", () => {
    const refresh = vi.fn();
    const watcher = watchChanges({ refresh, intervalMs: 1600, entity: "workspace-1", keepPolling: true });
    armChangeEvents({ push_events: true });
    vi.advanceTimersByTime(3200);
    expect(refresh).toHaveBeenCalledTimes(2);
    dispatchChangeEvent({ type: "entity.changed", id: "workspace-1" });
    expect(refresh).toHaveBeenCalledTimes(3);
    watcher.dispose();
    vi.advanceTimersByTime(1600);
    expect(refresh).toHaveBeenCalledTimes(3);
  });
  it("keeps a surface's own interval while unarmed", () => {
    expect(pollIntervalMs(1600)).toBe(1600);
  });

  it("stands a fast poll down to the safety poll once armed", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    expect(pollIntervalMs(1600)).toBe(SAFETY_POLL_MS);
    expect(SAFETY_POLL_MS).toBe(60000);
  });

  it("never speeds a slow poll up to the safety cadence", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    expect(pollIntervalMs(120000)).toBe(120000);
  });

  it("polls a watcher at its own interval while unarmed", () => {
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600 });
    vi.advanceTimersByTime(1600);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("polls an armed watcher only at the safety cadence", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600 });
    vi.advanceTimersByTime(59000);
    expect(refresh).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1000);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("re-times watchers that were already mounted when the mode changed", () => {
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600 });
    armChangeEvents({ push_events: true }, "dev-a");
    vi.advanceTimersByTime(1600);
    expect(refresh).not.toHaveBeenCalled();
    vi.advanceTimersByTime(58400);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("stops polling once disposed", () => {
    const refresh = vi.fn();
    const watcher = watchChanges({ refresh, intervalMs: 1000 });
    watcher.dispose();
    vi.advanceTimersByTime(5000);
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("board.changed", () => {
  it("refetches every board-scoped surface exactly once", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const feed = vi.fn();
    watchChanges({ refresh: feed, intervalMs: 2000 });
    dispatchChangeEvent({ type: "board.changed" }, "dev-a");
    expect(feed).toHaveBeenCalledTimes(1);
  });

  it("leaves entity surfaces alone — their own event says when they moved", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const detail = vi.fn();
    watchChanges({ refresh: detail, intervalMs: 1600, entity: "run-7" });
    dispatchChangeEvent({ type: "board.changed" }, "dev-a");
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
    watchChanges({ refresh: feed, intervalMs: 2000 });
    watchChanges({ refresh: detail, intervalMs: 1600, entity: "run-7" });

    expect(dispatchChangeEvent({ type: "session.hello" }, "dev-a")).toBe(false);
    expect(dispatchChangeEvent({ type: "entity.changed" }, "dev-a")).toBe(false);
    expect(dispatchChangeEvent({ type: "constructor" }, "dev-a")).toBe(false);
    expect(feed).not.toHaveBeenCalled();
    expect(detail).not.toHaveBeenCalled();
  });
});

describe("entity.changed", () => {
  it("refetches only the surfaces showing that entity", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const shown = vi.fn();
    const other = vi.fn();
    const feed = vi.fn();
    watchChanges({ refresh: shown, intervalMs: 1600, entity: "run-7" });
    watchChanges({ refresh: other, intervalMs: 1600, entity: "run-9" });
    watchChanges({ refresh: feed, intervalMs: 2000 });
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" }, "dev-a");
    expect(shown).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
    expect(feed).not.toHaveBeenCalled();
  });

  it("reads the entity at delivery, so a surface that moved is asked about now", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    let showing = "run-7";
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: () => showing });
    showing = "run-9";
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" }, "dev-a");
    expect(refresh).not.toHaveBeenCalled();
    dispatchChangeEvent({ type: "entity.changed", id: "run-9" }, "dev-a");
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("matches any of the ids a surface stands for", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: () => ["run-7", "wt-3"] });
    dispatchChangeEvent({ type: "entity.changed", id: "wt-3" }, "dev-a");
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("ignores an entity surface that does not know its id yet", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: () => null });
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" }, "dev-a");
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("an unarmed client", () => {
  it("ignores change events entirely", () => {
    const feed = vi.fn();
    const detail = vi.fn();
    watchChanges({ refresh: feed, intervalMs: 2000 });
    watchChanges({ refresh: detail, intervalMs: 1600, entity: "run-7" });
    dispatchChangeEvent({ type: "board.changed" }, "dev-a");
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" }, "dev-a");
    expect(feed).not.toHaveBeenCalled();
    expect(detail).not.toHaveBeenCalled();
  });
});

describe("a hidden tab", () => {
  it("skips the refetch an event asks for, exactly as it skips a poll", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000 });
    setHidden(true);
    dispatchChangeEvent({ type: "board.changed" }, "dev-a");
    expect(refresh).not.toHaveBeenCalled();
  });

  it("catches up on the events it missed the moment it comes back", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000 });
    setHidden(true);
    dispatchChangeEvent({ type: "board.changed" }, "dev-a");
    dispatchChangeEvent({ type: "board.changed" }, "dev-a");
    becomeVisible();
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("has nothing to catch up on when no event arrived while it was away", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000 });
    setHidden(true);
    becomeVisible();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("leaves the catch-up to a surface that owns its own", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000, catchUpOnVisible: false });
    setHidden(true);
    dispatchChangeEvent({ type: "board.changed" }, "dev-a");
    becomeVisible();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("still delivers to a surface whose poll does not pause while hidden", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: "iss-1", pausesWhileHidden: false });
    setHidden(true);
    dispatchChangeEvent({ type: "entity.changed", id: "iss-1" }, "dev-a");
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
      client: { name: "spa", version: expect.any(String), api_range: ">=1.0.0 <2.0.0" },
    });
    expect(call.mock.calls[0][1].client.version).not.toBe("");
  });

  it("remembers the bridge's api_version, and reads 0.0.0 from a bridge that reports none", async () => {
    expect(bridgeApiVersion()).toBe("0.0.0");
    await greetBridge(async () => ({ push_events: true, api_version: "1.0.0" }));
    expect(bridgeApiVersion()).toBe("1.0.0");
    await greetBridge(async () => ({ push_events: true }));
    expect(bridgeApiVersion()).toBe("0.0.0");
    await greetBridge(async () => ({ push_events: true, api_version: "1.2.0" }));
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    });
    expect(bridgeApiVersion()).toBe("0.0.0");
  });

  it("still hands the whole greeting, api_version included, to onGreeting", async () => {
    const accepted = vi.fn();
    const greeting = { push_events: true, api_version: "1.0.0" };
    await greetBridge(async () => greeting, { onGreeting: accepted });
    expect(accepted).toHaveBeenCalledWith(greeting);
  });

  it("leaves a bridge that refuses the greeting polling", async () => {
    const call = vi.fn(async () => {
      throw new Error("unknown method: session.hello");
    });
    await greetBridge(call, { deviceId: "dev-a" });
    expect(changeEventsArmed("dev-a")).toBe(false);
    expect(pollIntervalMs(1600)).toBe(1600);
  });

  it("requires an actual transport acknowledgement for a strict initial greeting", async () => {
    const dropped = new Error("your device went offline");
    await expect(greetBridge(async () => { throw dropped; }, { deviceId: "dev-a", strict: true })).rejects.toBe(dropped);
    await expect(greetBridge(async () => { throw new Error("unknown method: session.hello"); }, { strict: true })).resolves.toBe(false);
  });

  it("refetches every surface, whichever mode it lands in", async () => {
    const armedRefresh = vi.fn();
    watchChanges({ refresh: armedRefresh, intervalMs: 2000 });
    await greetBridge(async () => ({ push_events: true }), { deviceId: "dev-a" });
    expect(armedRefresh).toHaveBeenCalledTimes(1);

    resetChangeEvents();
    const pollingRefresh = vi.fn();
    watchChanges({ refresh: pollingRefresh, intervalMs: 2000 });
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
    watchChanges({ refresh: refreshed, intervalMs: 1600 });
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
    watchChanges({ refresh: feed, intervalMs: 2000 });
    watchChanges({ refresh: detail, intervalMs: 1600, entity: "run-7" });
    refetchEverything();
    expect(feed).toHaveBeenCalledTimes(1);
    expect(detail).toHaveBeenCalledTimes(1);
  });

  it("refetches while polling too — a poll-mode gap loses just as much", () => {
    const feed = vi.fn();
    watchChanges({ refresh: feed, intervalMs: 2000 });
    refetchEverything();
    expect(feed).toHaveBeenCalledTimes(1);
  });
});

// ---- more than one device --------------------------------------------------
// Every device pushes its own events down its own session. A watcher that named
// a device hears only that device; the merged inbox names none and hears them
// all, which is what makes one list out of several machines.
describe("several devices", () => {
  it("refreshes B's feed watcher and the merged inbox watcher on B's board.changed, not A's", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents({ push_events: true }, "dev-b");
    const feedA = vi.fn();
    const feedB = vi.fn();
    const inbox = vi.fn();
    watchChanges({ refresh: feedA, intervalMs: 2000, deviceId: "dev-a" });
    watchChanges({ refresh: feedB, intervalMs: 2000, deviceId: "dev-b" });
    watchChanges({ refresh: inbox, intervalMs: 2000 });

    dispatchChangeEvent({ type: "board.changed" }, "dev-b");

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
    watchChanges({ refresh: feedB, intervalMs: 2000, deviceId: "dev-b" });
    expect(dispatchChangeEvent({ type: "board.changed" }, "dev-b")).toBe(false);
    expect(feedB).not.toHaveBeenCalled();
  });

  it("stands the any-device cadence down only when every known device is armed", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents(null, "dev-b");
    expect(pollIntervalMs(1600)).toBe(1600); // dev-b still pushes nothing
    expect(pollIntervalMs(1600, "dev-a")).toBe(SAFETY_POLL_MS);

    armChangeEvents({ push_events: true }, "dev-b");
    expect(pollIntervalMs(1600)).toBe(SAFETY_POLL_MS);
  });

  it("re-times the any-device watchers when the last polling device arms", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents(null, "dev-b");
    const inbox = vi.fn();
    watchChanges({ refresh: inbox, intervalMs: 1600 });
    vi.advanceTimersByTime(1600);
    expect(inbox).toHaveBeenCalledTimes(1);

    armChangeEvents({ push_events: true }, "dev-b");
    vi.advanceTimersByTime(1600);
    expect(inbox).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(SAFETY_POLL_MS);
    expect(inbox).toHaveBeenCalledTimes(2);
  });

  it("forgets a device's armed state when it is disarmed", () => {
    armChangeEvents({ push_events: true }, "dev-a");
    armChangeEvents({ push_events: true }, "dev-b");
    disarmChangeEvents("dev-b");

    expect(changeEventsArmed("dev-b")).toBe(false);
    expect(changeEventsArmed("dev-a")).toBe(true);
    // The retired device is forgotten, not counted as a device that polls.
    expect(pollIntervalMs(1600)).toBe(SAFETY_POLL_MS);
    expect(dispatchChangeEvent({ type: "board.changed" }, "dev-b")).toBe(false);
  });

  it("arms the device greetBridge greeted and refetches that device's and the any-device watchers", async () => {
    const feedA = vi.fn();
    const feedB = vi.fn();
    const inbox = vi.fn();
    watchChanges({ refresh: feedA, intervalMs: 2000, deviceId: "dev-a" });
    watchChanges({ refresh: feedB, intervalMs: 2000, deviceId: "dev-b" });
    watchChanges({ refresh: inbox, intervalMs: 2000 });

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
    watchChanges({ refresh: feedA, intervalMs: 2000, deviceId: "dev-a" });
    watchChanges({ refresh: feedB, intervalMs: 2000, deviceId: "dev-b" });

    refetchEverything();

    expect(feedA).toHaveBeenCalledTimes(1);
    expect(feedB).toHaveBeenCalledTimes(1);
  });
});
