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
  api_version: "1.2.0",
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
    if (method === "session.hello") return { push_events: true };
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
    changeEvents.watchChanges({ refresh: () => {}, entity: "run-7", kinds: ["state", "git"] });
    await changeEvents.greetBridge(legacyBridge());
    await settle();
    expect(hellos()).toHaveLength(1);
    expect(hellos()[0].changes).toBeUndefined();
    expect(changeEvents.subscriptionsActive()).toBe(false);
    expect(subscribes()).toEqual([]);
  });

  it("falls back to legacy when a reconnect lands on a bridge without subscriptions", async () => {
    changeEvents.watchChanges({ refresh: () => {}, entity: "run-7", kinds: ["git"] });
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
    changeEvents.watchChanges({ refresh: () => {}, kinds: ["state", "git"] });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toEqual([
      expect.objectContaining({ scope: { kind: "board" }, kinds: ["state"] }),
    ]);
  });

  it("subscribes the background tier over the whole board at its own cadence", async () => {
    changeEvents.watchChanges({
      refresh: () => {},
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
    changeEvents.watchChanges({ refresh: () => {}, entity: "run-7" });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toEqual([]);
  });

  it("subscribes a surface mounted after the greeting, and unsubscribes it on dispose", async () => {
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    const watcher = changeEvents.watchChanges({
      refresh: () => {},
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
    changeEvents.watchChanges({ refresh: () => {}, entity: "run-7", kinds: ["git"] });
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
    changeEvents.watchChanges({ refresh: () => {}, entity: () => showing, kinds: ["git"] });
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
    expect(subscribes()).toEqual([]);
    showing = "run-9";
    changeEvents.refetchEverything();
    await settle();
    expect(subscribes()[0].scope).toEqual({ kind: "entity", id: "run-9" });
  });

  it("does not re-send a subscription that has not moved", async () => {
    changeEvents.watchChanges({ refresh: () => {}, entity: "run-7", kinds: ["git"] });
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
      entity: "run-7",
      kinds: ["git"],
      onChanges: (items) => seen.push(...items),
    });
    changeEvents.watchChanges({
      refresh: () => {},
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

  it("gives an all-scope watcher that reads state every item, the board's included", async () => {
    const seen = [];
    changeEvents.watchChanges({
      refresh: () => {},
      scope: "all",
      kinds: ["state", "thread"],
      mode: "realtime",
      priority: "foreground",
      onChanges: (items) => seen.push(...items),
    });
    await armed();
    changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s-inbox",
      items: [{ entity_id: "run-7", state: {} }, { entity_id: "board", state: { revision: 4 } }],
    });
    expect(seen).toEqual([{ entity_id: "run-7", state: {} }, { entity_id: "board", state: { revision: 4 } }]);
  });

  it("keeps the board item from an all-scope watcher that never asked for state", async () => {
    const seen = [];
    changeEvents.watchChanges({
      refresh: () => {},
      scope: "all",
      kinds: ["git", "files"],
      mode: { batch_ms: 30000 },
      priority: "background",
      onChanges: (items) => seen.push(...items),
    });
    await armed();
    changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s-background",
      items: [{ entity_id: "run-7", git: {} }, { entity_id: "board", state: { revision: 4 } }],
    });
    expect(seen).toEqual([{ entity_id: "run-7", git: {} }]);
  });

  it("gives a board-scoped watcher the board item", async () => {
    const seen = [];
    changeEvents.watchChanges({
      refresh: () => {},
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

  it("hands a flush to the subscription it names and to no other", async () => {
    // Three subscriptions cover the routed workspace at once (the sync layer
    // takes out exactly that shape), so a frame handed to everyone it covers
    // is applied three times and pulls three times as much.
    const background = [];
    const active = [];
    changeEvents.watchChanges({
      refresh: () => {},
      id: "s-background",
      scope: "all",
      kinds: ["git", "files"],
      mode: { batch_ms: 30000 },
      priority: "background",
      onChanges: (items) => background.push(...items),
    });
    changeEvents.watchChanges({
      refresh: () => {},
      id: "s-active",
      entity: "run-7",
      kinds: ["git", "files"],
      onChanges: (items) => active.push(...items),
    });
    await armed();

    changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s-active:run-7",
      items: [{ entity_id: "run-7", git: { status_key: "abc" } }],
    });

    expect(active).toEqual([{ entity_id: "run-7", git: { status_key: "abc" } }]);
    expect(background).toEqual([]);
  });

  it("keeps the kinds a watcher never subscribed to out of what it is handed", async () => {
    const seen = [];
    changeEvents.watchChanges({
      refresh: () => {},
      scope: "all",
      kinds: ["git"],
      mode: { batch_ms: 30000 },
      priority: "background",
      onChanges: (items) => seen.push(...items),
    });
    await armed();

    // A frame naming a subscription this client does not hold falls back to
    // who covers it — and a whole-board git watcher covers every entity.
    changeEvents.dispatchChangeEvent({
      type: "changes",
      subscription_id: "s-gone",
      items: [{ entity_id: "run-7", state: { state: "merged" }, git: { status_key: "abc" } }],
    });

    expect(seen).toEqual([{ entity_id: "run-7", git: { status_key: "abc" } }]);
  });

  it("runs the plain poll callback for a watcher that declared no onChanges", async () => {
    const refresh = vi.fn();
    changeEvents.watchChanges({ refresh, entity: "run-7", kinds: ["state", "git"] });
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
    changeEvents.watchChanges({ refresh, entity: "run-7", kinds: ["git"] });
    expect(changeEvents.dispatchChangeEvent({ type: "changes", items: [{ entity_id: "run-7" }] })).toBe(false);
    changeEvents.armChangeEvents({ push_events: true });
    expect(changeEvents.dispatchChangeEvent({ type: "changes", subscription_id: "s1", items: [] })).toBe(false);
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("the board revision", () => {
  const armed = async () => {
    await changeEvents.greetBridge(subscribingBridge());
    await settle();
  };

  const boardItem = (state) => ({
    type: "changes",
    subscription_id: "s-board",
    items: [{ entity_id: "board", ...(state ? { state } : {}) }],
  });

  const feedWatcher = () => {
    const refresh = vi.fn();
    changeEvents.watchChanges({ refresh, kinds: ["state"] });
    return refresh;
  };

  it("refreshes the feed once for a revision it has not seen", async () => {
    const refresh = feedWatcher();
    await armed();
    refresh.mockClear();

    changeEvents.dispatchChangeEvent(boardItem({ revision: 1183 }));
    changeEvents.dispatchChangeEvent(boardItem({ revision: 1183 }));

    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("refreshes again as soon as the revision moves", async () => {
    const refresh = feedWatcher();
    await armed();
    changeEvents.dispatchChangeEvent(boardItem({ revision: 1183 }));
    refresh.mockClear();

    changeEvents.dispatchChangeEvent(boardItem({ revision: 1184 }));

    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("refreshes on a board item that carries no revision at all", async () => {
    const refresh = feedWatcher();
    await armed();
    changeEvents.dispatchChangeEvent(boardItem({ revision: 1183 }));
    refresh.mockClear();

    changeEvents.dispatchChangeEvent(boardItem(null));
    changeEvents.dispatchChangeEvent(boardItem({}));

    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("leaves an entity watcher's own items alone", async () => {
    const seen = [];
    changeEvents.watchChanges({
      refresh: () => {},
      entity: "run-7",
      kinds: ["state"],
      onChanges: (items) => seen.push(...items),
    });
    await armed();
    const flush = {
      type: "changes",
      subscription_id: "s-bg",
      items: [{ entity_id: "run-7", state: {} }, { entity_id: "board", state: { revision: 5 } }],
    };
    changeEvents.dispatchChangeEvent(flush);
    changeEvents.dispatchChangeEvent(flush);

    expect(seen).toEqual([{ entity_id: "run-7", state: {} }, { entity_id: "run-7", state: {} }]);
  });

  it("refreshes on the first item after a reconnect, whatever the revision says", async () => {
    const refresh = feedWatcher();
    await armed();
    changeEvents.dispatchChangeEvent(boardItem({ revision: 1183 }));

    // A new session: the gap behind it announced nothing, and the bridge on
    // the other end may not be the one that counted to 1183.
    await armed();
    refresh.mockClear();
    changeEvents.dispatchChangeEvent(boardItem({ revision: 1183 }));

    expect(refresh).toHaveBeenCalledTimes(1);
  });
});

describe("telling the cache layer which contract is live", () => {
  // Each bridge negotiates for itself, so the flip names the machine it is
  // about: the cache layer re-times that device's reads and leaves the rest.
  it("announces the flip into subscriptions and back, naming the device it is about", async () => {
    const flips = [];
    const stop = changeEvents.onSubscriptionsChange((deviceId, active) => flips.push([deviceId, active]));
    await changeEvents.greetBridge(subscribingBridge(), { deviceId: "dev-1" });
    await changeEvents.greetBridge(legacyBridge(), { deviceId: "dev-1" });
    stop();
    expect(flips).toEqual([
      ["dev-1", true],
      ["dev-1", false],
    ]);
  });

  it("leaves the other device's contract standing when one falls back", async () => {
    const flips = [];
    const stop = changeEvents.onSubscriptionsChange((deviceId, active) => flips.push([deviceId, active]));
    await changeEvents.greetBridge(subscribingBridge(), { deviceId: "dev-1" });
    await changeEvents.greetBridge(subscribingBridge(), { deviceId: "dev-2" });
    await changeEvents.greetBridge(legacyBridge(), { deviceId: "dev-2" });
    stop();
    expect(flips).toEqual([
      ["dev-1", true],
      ["dev-2", true],
      ["dev-2", false],
    ]);
    expect(changeEvents.subscriptionsActive("dev-1")).toBe(true);
    expect(changeEvents.subscriptionsActive("dev-2")).toBe(false);
  });
});

/// One refused subscription used to be every subscription.
///
/// `changes.subscribe` carries a whole spec, so one unknown field refuses the
/// call — and the loop that took the subscriptions out returned on the first
/// refusal. A client that named a kind its bridge had never heard of therefore
/// held NO subscriptions at all: not the refused one, and not the two after
/// it. It looked exactly like a dead connection — nothing pushed, and the only
/// thing that painted was what the reader typed. That is what this pins.
describe("a bridge that refuses one subscription", () => {
  /** A bridge that knows every kind but one, the way a bridge predating a new
   *  kind answers: `invalid_params` for the spec that names it. */
  const pickyBridge = (unknownKind) => {
    call = vi.fn(async (method, params) => {
      calls.push([method, params]);
      if (method === "session.hello") {
        const mode = params.changes === "subscriptions" ? "subscriptions" : "legacy";
        return { ...SUBSCRIBING_GREETING, changes: { ...SUBSCRIBING_GREETING.changes, mode } };
      }
      if (method === "changes.subscribe") {
        if ((params.kinds || []).includes(unknownKind)) {
          throw new Error(`unknown variant \`${unknownKind}\``);
        }
        return { subscription_id: params.subscription_id, watch: "live" };
      }
      if (method === "changes.unsubscribe") return { ok: true };
      return {};
    });
    return call;
  };

  const threeWatchers = () => {
    changeEvents.watchChanges({ refresh: () => {}, id: "s-inbox", scope: "all", kinds: ["state", "thread", "tasks"], mode: "realtime" });
    changeEvents.watchChanges({ refresh: () => {}, id: "s-background", scope: "all", kinds: ["git", "files"], mode: { batch_ms: 30000 } });
    changeEvents.watchChanges({ refresh: () => {}, id: "s-active", entity: "run-7", kinds: ["git", "files"], mode: "realtime" });
  };

  it("still takes out every other one", async () => {
    threeWatchers();
    await changeEvents.greetBridge(pickyBridge("tasks"));
    await settle();

    const taken = subscribes().map((spec) => spec.subscription_id);
    expect(taken).toContain("s-inbox"); // asked for, and refused
    expect(taken).toContain("s-background");
    expect(taken).toContain("s-active:run-7");
  });

  it("asks for the refused one again on the next diff, so an upgraded bridge heals", async () => {
    threeWatchers();
    await changeEvents.greetBridge(pickyBridge("tasks"));
    await settle();
    const before = subscribes().filter((spec) => spec.subscription_id === "s-inbox").length;

    // Any later diff: another watcher mounting is one.
    changeEvents.watchChanges({ refresh: () => {}, entity: "run-9", kinds: ["git"] });
    await settle();

    expect(subscribes().filter((spec) => spec.subscription_id === "s-inbox").length).toBeGreaterThan(before);
  });

  it("holds the ones that worked, and does not re-ask for them", async () => {
    threeWatchers();
    await changeEvents.greetBridge(pickyBridge("tasks"));
    await settle();
    const before = subscribes().filter((spec) => spec.subscription_id === "s-background").length;

    changeEvents.watchChanges({ refresh: () => {}, entity: "run-9", kinds: ["git"] });
    await settle();

    expect(subscribes().filter((spec) => spec.subscription_id === "s-background").length).toBe(before);
  });

  it("stops the diff when the SESSION goes, rather than when a spec is refused", async () => {
    threeWatchers();
    let replaced = false;
    const dying = vi.fn(async (method, params) => {
      calls.push([method, params]);
      if (method === "session.hello") {
        const mode = params.changes === "subscriptions" ? "subscriptions" : "legacy";
        return { ...SUBSCRIBING_GREETING, changes: { ...SUBSCRIBING_GREETING.changes, mode } };
      }
      if (method === "changes.subscribe") {
        // The first subscribe lands; the session is replaced under the second.
        if (!replaced) { replaced = true; return { subscription_id: params.subscription_id, watch: "live" }; }
        await changeEvents.greetBridge(subscribingBridge());
        return { subscription_id: params.subscription_id, watch: "live" };
      }
      return {};
    });
    await changeEvents.greetBridge(dying);
    await settle();

    // The replacement session replays the whole map, so every id is taken out
    // on it — the point is that the dead session stopped rather than carrying on.
    const onLive = subscribes().map((spec) => spec.subscription_id);
    expect(new Set(onLive)).toEqual(new Set(["s-inbox", "s-background", "s-active:run-7"]));
  });
});

/// A refused subscribe is the failure nobody can see from inside the app: the
/// client asks, the bridge says no, and the page never hears another thing.
/// It goes on the connection diagnostic record so Settings → Diagnostics can
/// show it — on a phone that is the only place it CAN be read.
describe("a refused subscribe on the record", () => {
  let diagnostics;

  beforeEach(async () => {
    diagnostics = await import("../src/core/connectionDiagnostics.js");
    diagnostics.clearConnectionDiagnosticHistory();
  });

  const refusing = (code) => {
    call = vi.fn(async (method, params) => {
      calls.push([method, params]);
      if (method === "session.hello") {
        const mode = params.changes === "subscriptions" ? "subscriptions" : "legacy";
        return { ...SUBSCRIBING_GREETING, changes: { ...SUBSCRIBING_GREETING.changes, mode } };
      }
      if (method === "changes.subscribe") {
        const refusal = new Error("unknown variant `tasks`");
        if (code) refusal.error_code = code;
        throw refusal;
      }
      return {};
    });
    return call;
  };

  const subscriptionEntries = () =>
    diagnostics.connectionDiagnosticHistory().filter((entry) => entry.event === "subscription");

  it("names the subscription and the bridge's code", async () => {
    changeEvents.watchChanges({ refresh: () => {}, id: "s-inbox", scope: "all", kinds: ["state", "thread"], mode: "realtime" });
    await changeEvents.greetBridge(refusing("invalid_params"), { deviceId: "dev-1" });
    await settle();

    const [entry] = subscriptionEntries();
    expect(entry).toMatchObject({ state: "refused", subscription: "s-inbox", code: "invalid_params" });
    // The machine stays readable off the id, the way every other diagnostic is.
    expect(String(entry.connection).startsWith("dev-1:")).toBe(true);
  });

  it("says unknown for a refusal that named no code", async () => {
    changeEvents.watchChanges({ refresh: () => {}, id: "s-inbox", scope: "all", kinds: ["state"], mode: "realtime" });
    await changeEvents.greetBridge(refusing(null), { deviceId: "dev-1" });
    await settle();

    expect(subscriptionEntries()[0].code).toBe("unknown");
  });

  it("records nothing for a subscribe that worked", async () => {
    changeEvents.watchChanges({ refresh: () => {}, id: "s-inbox", scope: "all", kinds: ["state"], mode: "realtime" });
    await changeEvents.greetBridge(subscribingBridge(), { deviceId: "dev-1" });
    await settle();

    expect(subscriptionEntries()).toEqual([]);
  });
});
