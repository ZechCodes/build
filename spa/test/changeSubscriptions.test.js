// @vitest-environment jsdom
// Push subscriptions, client side (wire spec step 1.6). A surface registers
// what it wants — scope, kinds, cadence, priority — and the manager keeps the
// bridge's subscription set equal to that map: on mount, on unmount, on greet,
// and on every reconnect. A bridge that does not advertise subscriptions is
// left in legacy mode with today's poll behaviour, unchanged.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let changeEvents;
let call;
let calls;

const SUBSCRIBING_GREETING = {
  push_events: true,
  api_version: "1.1.0",
  events: ["board.changed", "entity.changed", "changes"],
  changes: {
    subscriptions: true,
    mode: "legacy",
    kinds: ["state", "thread", "git", "files"],
    batch_ms: { min: 1000, max: 600000 },
  },
};

const subscribingBridge = () => {
  call = vi.fn(async (method, params) => {
    calls.push([method, params]);
    if (method === "session.hello") {
      const mode = params.changes === "subscriptions" ? "subscriptions" : "legacy";
      return { ...SUBSCRIBING_GREETING, changes: { ...SUBSCRIBING_GREETING.changes, mode } };
    }
    if (method === "changes.subscribe") return { subscription_id: params.subscription_id, watch: "live" };
    if (method === "changes.unsubscribe") return { ok: true };
    return {};
  });
  return call;
};

const legacyBridge = () => {
  call = vi.fn(async (method, params) => {
    calls.push([method, params]);
    if (method === "session.hello") return { push_events: true, api_version: "1.0.0" };
    return {};
  });
  return call;
};

const subscribes = () => calls.filter(([method]) => method === "changes.subscribe").map(([, params]) => params);
const unsubscribes = () => calls.filter(([method]) => method === "changes.unsubscribe").map(([, p]) => p.subscription_id);
const hellos = () => calls.filter(([method]) => method === "session.hello").map(([, params]) => params);

const settle = async () => {
  await changeEvents.subscriptionsSettled();
};

beforeEach(async () => {
  vi.resetModules();
  calls = [];
  changeEvents = await import("../src/core/changeEvents.js");
});

afterEach(() => {
  changeEvents.resetChangeEvents();
});

describe("negotiating the contract", () => {
  it("re-greets in subscriptions mode once the greeting advertises them", async () => {
    await changeEvents.greetBridge(subscribingBridge());
    expect(hellos()).toHaveLength(2);
    expect(hellos()[0].changes).toBeUndefined();
    expect(hellos()[1].changes).toBe("subscriptions");
    expect(changeEvents.subscriptionsActive()).toBe(true);
  });

  it("greets in subscriptions mode straight away on the next session", async () => {
    await changeEvents.greetBridge(subscribingBridge());
    calls.length = 0;
    await changeEvents.greetBridge(call);
    expect(hellos()).toHaveLength(1);
    expect(hellos()[0].changes).toBe("subscriptions");
  });

  it("stays legacy against a bridge that advertises nothing, and subscribes to nothing", async () => {
    changeEvents.watchChanges({ refresh: () => {}, intervalMs: 1600, entity: "run-7", kinds: ["state", "git"] });
    await changeEvents.greetBridge(legacyBridge());
    await settle();
    expect(hellos()).toHaveLength(1);
    expect(hellos()[0].changes).toBeUndefined();
    expect(changeEvents.subscriptionsActive()).toBe(false);
    expect(subscribes()).toEqual([]);
  });

  it("falls back to legacy when a reconnect lands on a bridge without subscriptions", async () => {
    changeEvents.watchChanges({ refresh: () => {}, intervalMs: 1600, entity: "run-7", kinds: ["git"] });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    // The old bridge ignores the field it does not know and answers legacy.
    calls.length = 0;
    await changeEvents.greetBridge(legacyBridge());
    await settle();
    expect(changeEvents.subscriptionsActive()).toBe(false);
    expect(subscribes()).toEqual([]);
  });
});

describe("the desired map", () => {
  it("subscribes a mounted entity surface with its kinds, mode and priority", async () => {
    changeEvents.watchChanges({
      refresh: () => {},
      intervalMs: 1600,
      entity: () => ["run-7", "wt-3"],
      kinds: ["state", "thread", "git", "files"],
      mode: "realtime",
    });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toEqual([
      expect.objectContaining({
        scope: { kind: "entity", id: "run-7" },
        kinds: ["state", "thread", "git", "files"],
        mode: "realtime",
        priority: "foreground",
      }),
      expect.objectContaining({ scope: { kind: "entity", id: "wt-3" } }),
    ]);
  });

  it("keeps a board-scoped surface to state, which is all board scope accepts", async () => {
    changeEvents.watchChanges({ refresh: () => {}, intervalMs: 2000, kinds: ["state", "git"] });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toEqual([
      expect.objectContaining({ scope: { kind: "board" }, kinds: ["state"] }),
    ]);
  });

  it("subscribes the background tier over the whole board at its own cadence", async () => {
    changeEvents.watchChanges({
      refresh: () => {},
      intervalMs: 600000,
      scope: "all",
      kinds: ["files"],
      mode: { batch_ms: 180000 },
      priority: "background",
      onChanges: () => {},
    });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()[0]).toMatchObject({
      scope: { kind: "all" },
      kinds: ["files"],
      mode: { batch_ms: 180000 },
      priority: "background",
    });
  });

  it("asks for nothing on behalf of a surface that named no kinds", async () => {
    changeEvents.watchChanges({ refresh: () => {}, intervalMs: 1600, entity: "run-7" });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toEqual([]);
  });

  it("subscribes a surface mounted after the greeting, and unsubscribes it on dispose", async () => {
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    const watcher = changeEvents.watchChanges({
      refresh: () => {},
      intervalMs: 1600,
      entity: "run-7",
      kinds: ["git"],
    });
    await settle();
    const id = subscribes()[0].subscription_id;
    expect(id).toBeTruthy();
    watcher.dispose();
    await settle();
    expect(unsubscribes()).toEqual([id]);
  });

  it("replays the whole map after a reconnect, since the new session holds none of it", async () => {
    changeEvents.watchChanges({ refresh: () => {}, intervalMs: 1600, entity: "run-7", kinds: ["git"] });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toHaveLength(1);
    await changeEvents.greetBridge(call);
    await settle();
    expect(subscribes()).toHaveLength(2);
    expect(unsubscribes()).toEqual([]);
  });

  it("re-subscribes the same id when a surface's entity id resolves late", async () => {
    let showing = null;
    changeEvents.watchChanges({ refresh: () => {}, intervalMs: 1600, entity: () => showing, kinds: ["git"] });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toEqual([]);
    showing = "run-9";
    changeEvents.refetchEverything();
    await settle();
    expect(subscribes()[0].scope).toEqual({ kind: "entity", id: "run-9" });
  });

  it("does not re-send a subscription that has not moved", async () => {
    changeEvents.watchChanges({ refresh: () => {}, intervalMs: 1600, entity: "run-7", kinds: ["git"] });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    changeEvents.refetchEverything();
    await settle();
    expect(subscribes()).toHaveLength(1);
  });
});

describe("the changes event", () => {
  const armed = async () => {
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
  };

  it("hands each watcher the items for the entities it is showing", async () => {
    const seen = [];
    const other = vi.fn();
    changeEvents.watchChanges({
      refresh: () => {},
      intervalMs: 1600,
      entity: "run-7",
      kinds: ["git"],
      onChanges: (items) => seen.push(...items),
    });
    changeEvents.watchChanges({
      refresh: () => {},
      intervalMs: 1600,
      entity: "run-9",
      kinds: ["git"],
      onChanges: other,
    });
    await armed();

    const handled = changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s1",
      items: [
        { entity_id: "run-7", git: { status_key: "abc", head: "def" } },
        { entity_id: "run-42", state: {} },
      ],
    });

    expect(handled).toBe(true);
    expect(seen).toEqual([{ entity_id: "run-7", git: { status_key: "abc", head: "def" } }]);
    expect(other).not.toHaveBeenCalled();
  });

  it("gives an all-scope watcher every entity item and no board item", async () => {
    const seen = [];
    changeEvents.watchChanges({
      refresh: () => {},
      intervalMs: 600000,
      scope: "all",
      kinds: ["state"],
      mode: { batch_ms: 30000 },
      priority: "background",
      onChanges: (items) => seen.push(...items),
    });
    await armed();
    changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s-bg",
      items: [{ entity_id: "run-7", state: {} }, { entity_id: "board", state: { revision: 4 } }],
    });
    expect(seen).toEqual([{ entity_id: "run-7", state: {} }]);
  });

  it("gives a board-scoped watcher the board item", async () => {
    const seen = [];
    changeEvents.watchChanges({
      refresh: () => {},
      intervalMs: 2000,
      kinds: ["state"],
      onChanges: (items) => seen.push(...items),
    });
    await armed();
    changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s-board",
      items: [{ entity_id: "run-7", state: {} }, { entity_id: "board", state: { revision: 4 } }],
    });
    expect(seen).toEqual([{ entity_id: "board", state: { revision: 4 } }]);
  });

  it("runs the plain poll callback for a watcher that declared no onChanges", async () => {
    const refresh = vi.fn();
    changeEvents.watchChanges({ refresh, intervalMs: 1600, entity: "run-7", kinds: ["state", "git"] });
    await armed();
    refresh.mockClear();
    changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s1",
      items: [{ entity_id: "run-7", git: { status_key: "abc" } }],
    });
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("ignores an event with nothing in it, and one that arrived unarmed", () => {
    const refresh = vi.fn();
    changeEvents.watchChanges({ refresh, intervalMs: 1600, entity: "run-7", kinds: ["git"] });
    expect(changeEvents.dispatchChangeEvent({ type: "changes", items: [{ entity_id: "run-7" }] })).toBe(false);
    changeEvents.armChangeEvents({ push_events: true });
    expect(changeEvents.dispatchChangeEvent({ type: "changes", subscription_id: "s1", items: [] })).toBe(false);
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("telling the cache layer which contract is live", () => {
  it("announces the flip into subscriptions and back", async () => {
    const flips = [];
    const stop = changeEvents.onSubscriptionsChange((active) => flips.push(active));
    await changeEvents.greetBridge(subscribingBridge());
    await changeEvents.greetBridge(legacyBridge());
    stop();
    expect(flips).toEqual([true, false]);
  });
});
