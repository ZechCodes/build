// @vitest-environment jsdom
// Push invalidation, client side: the bridge says what moved, and the surface
// showing it refetches. Everything here is about the two modes being separate —
// an old bridge never arms, and an unarmed client polls exactly as it always
// did.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let armChangeEvents,
  changeEventsArmed,
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
    changeEventsArmed,
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
    expect(armChangeEvents({ push_events: true, events: ["board.changed"] })).toBe(true);
    expect(changeEventsArmed()).toBe(true);
  });

  it("stays unarmed for a bridge that never answered the greeting", () => {
    expect(armChangeEvents(null)).toBe(false);
    expect(changeEventsArmed()).toBe(false);
  });

  it("stays unarmed for a greeting without the flag", () => {
    expect(armChangeEvents({ pong: true })).toBe(false);
    expect(armChangeEvents({ push_events: false })).toBe(false);
    expect(changeEventsArmed()).toBe(false);
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
    armChangeEvents({ push_events: true });
    expect(pollIntervalMs(1600)).toBe(SAFETY_POLL_MS);
    expect(SAFETY_POLL_MS).toBe(60000);
  });

  it("never speeds a slow poll up to the safety cadence", () => {
    armChangeEvents({ push_events: true });
    expect(pollIntervalMs(120000)).toBe(120000);
  });

  it("polls a watcher at its own interval while unarmed", () => {
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600 });
    vi.advanceTimersByTime(1600);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("polls an armed watcher only at the safety cadence", () => {
    armChangeEvents({ push_events: true });
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
    armChangeEvents({ push_events: true });
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
    armChangeEvents({ push_events: true });
    const feed = vi.fn();
    watchChanges({ refresh: feed, intervalMs: 2000 });
    dispatchChangeEvent({ type: "board.changed" });
    expect(feed).toHaveBeenCalledTimes(1);
  });

  it("leaves entity surfaces alone — their own event says when they moved", () => {
    armChangeEvents({ push_events: true });
    const detail = vi.fn();
    watchChanges({ refresh: detail, intervalMs: 1600, entity: "run-7" });
    dispatchChangeEvent({ type: "board.changed" });
    expect(detail).not.toHaveBeenCalled();
  });
});

describe("entity.changed", () => {
  it("refetches only the surfaces showing that entity", () => {
    armChangeEvents({ push_events: true });
    const shown = vi.fn();
    const other = vi.fn();
    const feed = vi.fn();
    watchChanges({ refresh: shown, intervalMs: 1600, entity: "run-7" });
    watchChanges({ refresh: other, intervalMs: 1600, entity: "run-9" });
    watchChanges({ refresh: feed, intervalMs: 2000 });
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" });
    expect(shown).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
    expect(feed).not.toHaveBeenCalled();
  });

  it("reads the entity at delivery, so a surface that moved is asked about now", () => {
    armChangeEvents({ push_events: true });
    let showing = "run-7";
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: () => showing });
    showing = "run-9";
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" });
    expect(refresh).not.toHaveBeenCalled();
    dispatchChangeEvent({ type: "entity.changed", id: "run-9" });
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("matches any of the ids a surface stands for", () => {
    armChangeEvents({ push_events: true });
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: () => ["run-7", "wt-3"] });
    dispatchChangeEvent({ type: "entity.changed", id: "wt-3" });
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("ignores an entity surface that does not know its id yet", () => {
    armChangeEvents({ push_events: true });
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: () => null });
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" });
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("an unarmed client", () => {
  it("ignores change events entirely", () => {
    const feed = vi.fn();
    const detail = vi.fn();
    watchChanges({ refresh: feed, intervalMs: 2000 });
    watchChanges({ refresh: detail, intervalMs: 1600, entity: "run-7" });
    dispatchChangeEvent({ type: "board.changed" });
    dispatchChangeEvent({ type: "entity.changed", id: "run-7" });
    expect(feed).not.toHaveBeenCalled();
    expect(detail).not.toHaveBeenCalled();
  });
});

describe("a hidden tab", () => {
  it("skips the refetch an event asks for, exactly as it skips a poll", () => {
    armChangeEvents({ push_events: true });
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000 });
    setHidden(true);
    dispatchChangeEvent({ type: "board.changed" });
    expect(refresh).not.toHaveBeenCalled();
  });

  it("catches up on the events it missed the moment it comes back", () => {
    armChangeEvents({ push_events: true });
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000 });
    setHidden(true);
    dispatchChangeEvent({ type: "board.changed" });
    dispatchChangeEvent({ type: "board.changed" });
    becomeVisible();
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("has nothing to catch up on when no event arrived while it was away", () => {
    armChangeEvents({ push_events: true });
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000 });
    setHidden(true);
    becomeVisible();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("leaves the catch-up to a surface that owns its own", () => {
    armChangeEvents({ push_events: true });
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 2000, catchUpOnVisible: false });
    setHidden(true);
    dispatchChangeEvent({ type: "board.changed" });
    becomeVisible();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("still delivers to a surface whose poll does not pause while hidden", () => {
    armChangeEvents({ push_events: true });
    const refresh = vi.fn();
    watchChanges({ refresh, intervalMs: 1600, entity: "iss-1", pausesWhileHidden: false });
    setHidden(true);
    dispatchChangeEvent({ type: "entity.changed", id: "iss-1" });
    expect(refresh).toHaveBeenCalledTimes(1);
  });
});

describe("the greeting", () => {
  it("arms event mode when the bridge answers the flag", async () => {
    const call = vi.fn(async () => ({ push_events: true, events: ["board.changed"] }));
    await greetBridge(call);
    expect(call).toHaveBeenCalledWith("session.hello");
    expect(changeEventsArmed()).toBe(true);
  });

  it("leaves a bridge that refuses the greeting polling", async () => {
    const call = vi.fn(async () => {
      throw new Error("unknown method: session.hello");
    });
    await greetBridge(call);
    expect(changeEventsArmed()).toBe(false);
    expect(pollIntervalMs(1600)).toBe(1600);
  });

  it("refetches every surface, whichever mode it lands in", async () => {
    const armedRefresh = vi.fn();
    watchChanges({ refresh: armedRefresh, intervalMs: 2000 });
    await greetBridge(async () => ({ push_events: true }));
    expect(armedRefresh).toHaveBeenCalledTimes(1);

    resetChangeEvents();
    const pollingRefresh = vi.fn();
    watchChanges({ refresh: pollingRefresh, intervalMs: 2000 });
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    });
    expect(pollingRefresh).toHaveBeenCalledTimes(1);
  });

  it("disarms when a reconnect lands on a bridge that cannot push", async () => {
    await greetBridge(async () => ({ push_events: true }));
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    });
    expect(changeEventsArmed()).toBe(false);
  });

  it("ignores a late greeting after its session stops owning the application", async () => {
    let resolveGreeting;
    let current = true;
    const refreshed = vi.fn();
    const accepted = vi.fn();
    watchChanges({ refresh: refreshed, intervalMs: 1600 });
    const pending = greetBridge(
      () => new Promise((resolve) => { resolveGreeting = resolve; }),
      { isCurrent: () => current, onGreeting: accepted },
    );
    current = false;
    resolveGreeting({ push_events: true, thread_post_operations: { version: 1, status_method: "operation.get" } });
    await pending;

    expect(changeEventsArmed()).toBe(false);
    expect(refreshed).not.toHaveBeenCalled();
    expect(accepted).not.toHaveBeenCalled();
  });
});

describe("a reconnect", () => {
  it("refetches every surface once — the gap announced nothing", () => {
    armChangeEvents({ push_events: true });
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
