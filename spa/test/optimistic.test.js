import { describe, it, expect, beforeEach, vi } from "vitest";

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notifySuccess: () => {} }));

const {
  PENDING_GRACE_MS,
  createOptimisticStore,
  insertRecord,
  removeRecord,
  patchRecord,
  projectPending,
  retirePending,
  pendingIn,
  isPending,
  projectOptimistic,
  provisionalKey,
  isProvisionalKey,
  reconcileOptimistic,
  resetOptimistic,
  runOptimistic,
  subscribeOptimistic,
} = await import("../src/core/optimistic.js");

const keyOf = (entry) => entry.id;
const agent = (id, over = {}) => ({ id, ordinal: 1, ...over });
const settled = (record, settledAt) => ({ ...record, settledAt });

beforeEach(() => {
  resetOptimistic();
  notifyError.mockClear();
});

describe("the records and the projection", () => {
  it("builds plain records and projects them onto a snapshot", () => {
    const records = [
      insertRecord("c", agent("c")),
      removeRecord("a"),
      patchRecord("b", { working: true }),
    ];
    expect(records[0]).toEqual({ kind: "insert", key: "c", entry: agent("c"), scope: null, settledAt: null });
    expect(records[1]).toEqual({ kind: "remove", key: "a", scope: null, settledAt: null });
    expect(records[2]).toEqual({
      kind: "patch",
      key: "b",
      fields: { working: true },
      scope: null,
      clearedBy: expect.any(Function),
      settledAt: null,
    });

    const projected = projectPending([agent("a"), agent("b")], records, { keyOf });
    expect(projected.map(keyOf)).toEqual(["b", "c"]);
    expect(projected[0].working).toBe(true);
  });

  it("never mutates the snapshot it projects", () => {
    const first = agent("a");
    const second = agent("b");
    const entries = [first, second];
    projectPending(
      entries,
      [insertRecord("c", agent("c")), removeRecord("a"), patchRecord("b", { working: true })],
      { keyOf },
    );
    expect(entries).toHaveLength(2);
    expect(entries[0]).toBe(first);
    expect(entries[1]).toBe(second);
    expect(first).toEqual({ id: "a", ordinal: 1 });
    expect(second).toEqual({ id: "b", ordinal: 1 });
  });

  it("neither wipes nor duplicates when a fresh snapshot lands mid-flight", () => {
    const records = [insertRecord("c", agent("c"))];
    expect(projectPending([agent("a")], records, { keyOf }).map(keyOf)).toEqual(["a", "c"]);
    expect(projectPending([agent("a"), agent("b")], records, { keyOf }).map(keyOf)).toEqual(["a", "b", "c"]);

    const carrying = projectPending([agent("a"), agent("c", { real: true })], records, { keyOf });
    expect(carrying).toHaveLength(2);
    expect(carrying.map(keyOf)).toEqual(["a", "c"]);
    expect(carrying[1].real).toBe(true);
  });

  it("retires each kind only when the snapshot agrees", () => {
    const nowMs = 1000;
    const remove = settled(removeRecord("a"), nowMs);
    expect(retirePending([remove], [agent("a")], { keyOf, nowMs })).toEqual([remove]);
    expect(retirePending([remove], [agent("b")], { keyOf, nowMs })).toEqual([]);

    const insert = settled(insertRecord("c", agent("c")), nowMs);
    expect(retirePending([insert], [agent("a")], { keyOf, nowMs })).toEqual([insert]);
    expect(retirePending([insert], [agent("c")], { keyOf, nowMs })).toEqual([]);

    const patch = settled(patchRecord("b", { working: true }), nowMs);
    expect(retirePending([patch], [agent("b")], { keyOf, nowMs })).toEqual([patch]);
    expect(retirePending([patch], [agent("b", { working: true })], { keyOf, nowMs })).toEqual([]);

    const unsettled = [removeRecord("a"), insertRecord("c", agent("c")), patchRecord("b", { working: true })];
    expect(retirePending(unsettled, [agent("b", { working: true }), agent("c")], { keyOf, nowMs })).toEqual(unsettled);
  });

  it("retires a settled patch once its key has left the snapshot", () => {
    const nowMs = 1000;
    const patch = settled(patchRecord("b", { working: true }), nowMs);
    expect(retirePending([patch], [agent("a")], { keyOf, nowMs })).toEqual([]);
    expect(retirePending([patchRecord("b", { working: true })], [agent("a")], { keyOf, nowMs })).toEqual([
      patchRecord("b", { working: true }),
    ]);
  });

  // A reply that answers before the work behind it is done (the bridge spawns a
  // harness behind its own answer) leaves the row saying something the entity
  // will never carry as a field. Such a patch names what settles it instead,
  // and stands until the pushed entry says so.
  it("keeps a patch the entry cannot carry until its own answer arrives", () => {
    const nowMs = 1000;
    const isLive = (entry) => entry.state === "live";
    const starting = settled(patchRecord("b", { state: "starting" }, { clearedBy: isLive }), nowMs);
    expect(retirePending([starting], [agent("b", { state: "idle" })], { keyOf, nowMs })).toEqual([starting]);
    expect(retirePending([starting], [agent("b", { state: "starting" })], { keyOf, nowMs })).toEqual([starting]);
    expect(retirePending([starting], [agent("b", { state: "live" })], { keyOf, nowMs })).toEqual([]);
  });

  it("lets a settled record go once the grace runs out", () => {
    const insert = settled(insertRecord("c", agent("c")), 1000);
    const held = { keyOf, nowMs: 1000 + PENDING_GRACE_MS };
    expect(retirePending([insert], [agent("a")], held)).toEqual([insert]);
    expect(retirePending([insert], [agent("a")], { keyOf, nowMs: 1001 + PENDING_GRACE_MS })).toEqual([]);
  });
});

describe("the registry", () => {
  it("keeps independent stores isolated, including late failures", async () => {
    const first = createOptimisticStore();
    const second = createOptimisticStore();
    const firstNotifications = vi.fn();
    const secondNotifications = vi.fn();
    first.subscribeOptimistic("agents", firstNotifications);
    second.subscribeOptimistic("agents", secondNotifications);

    let rejectFirst;
    const firstRun = first.runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-agent", agent("pending-agent"))],
      call: () => new Promise((_, reject) => {
        rejectFirst = reject;
      }),
      failureSummary: "Could not start the agent",
      notify: false,
    });
    await second.runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-agent", agent("pending-agent"))],
      call: async () => {},
      failureSummary: "Could not start the agent",
      notify: false,
    });

    expect(first.pendingIn("agents")).toHaveLength(1);
    expect(second.pendingIn("agents")).toHaveLength(1);
    expect(firstNotifications).toHaveBeenCalled();
    expect(secondNotifications).toHaveBeenCalled();
    expect(first.provisionalKey("message")).not.toBe(second.provisionalKey("message"));

    rejectFirst(new Error("nope"));
    await expect(firstRun).resolves.toBe(false);
    expect(first.pendingIn("agents")).toEqual([]);
    expect(second.pendingIn("agents")).toHaveLength(1);
  });

  it("does not reuse a store's provisional keys after reset", () => {
    const store = createOptimisticStore();
    const first = store.provisionalKey("agent");
    store.reset();
    expect(store.provisionalKey("agent")).not.toBe(first);
  });

  it("paints before the call answers, and settles after", async () => {
    const notified = [];
    const unsubscribe = subscribeOptimistic("agents", () => notified.push(pendingIn("agents").length));
    let handed = null;
    let seenInside = null;

    const result = await runOptimistic({
      scope: "agents",
      records: [insertRecord("c", agent("c"))],
      call: async (handle) => {
        handed = handle;
        seenInside = projectOptimistic("agents", [agent("a")], { keyOf }).map(keyOf);
      },
      failureSummary: "Could not do it",
    });

    expect(seenInside).toEqual(["a", "c"]);
    expect(result).toBe(true);
    expect(typeof handed.rekey).toBe("function");
    expect(typeof handed.moveScope).toBe("function");
    expect(typeof handed.drop).toBe("function");
    expect(notified.length).toBeGreaterThanOrEqual(2);
    expect(pendingIn("agents")[0].settledAt).toBeGreaterThan(0);
    unsubscribe();
  });

  it("puts everything back and says why when the call is refused", async () => {
    const onRevert = vi.fn();
    const result = await runOptimistic({
      scope: "agents",
      records: [insertRecord("c", agent("c"))],
      call: async () => {
        throw new Error("nope");
      },
      failureSummary: "Could not start the agent",
      onRevert,
    });

    expect(result).toBe(false);
    expect(pendingIn("agents")).toEqual([]);
    expect(onRevert).toHaveBeenCalledTimes(1);
    expect(onRevert.mock.calls[0][0].message).toBe("nope");
    expect(notifyError).toHaveBeenCalledTimes(1);
    expect(notifyError).toHaveBeenCalledWith("Could not start the agent", "nope");
  });

  it("renames a record to the identity the answer gave it", async () => {
    await runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-agent-1", agent("pending-agent-1"))],
      call: async (handle) => handle.rekey("pending-agent-1", "ag-2"),
      failureSummary: "Could not start the agent",
    });

    expect(pendingIn("agents").map((record) => record.key)).toEqual(["ag-2"]);
    expect(projectOptimistic("agents", [], { keyOf })).toHaveLength(1);
    reconcileOptimistic("agents", [agent("ag-2")], { keyOf });
    expect(pendingIn("agents")).toEqual([]);
  });

  it("carries the entry's own identity across a rekey", async () => {
    await runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-agent-1", agent("pending-agent-1"))],
      call: async (handle) => handle.rekey("pending-agent-1", "ag-2", agent("ag-2", { ordinal: 2 })),
      failureSummary: "Could not start the agent",
    });

    const projected = projectOptimistic("agents", [], { keyOf });
    expect(projected.map(keyOf)).toEqual(["ag-2"]);
    expect(keyOf(projected[0])).toBe(pendingIn("agents")[0].key);
    expect(projected[0].ordinal).toBe(2);
  });

  it("keeps a record the answer already confirmed", async () => {
    const result = await runOptimistic({
      scope: "agents",
      records: [
        insertRecord("pending-agent-1", agent("pending-agent-1")),
        insertRecord("pending-message-2", { id: "pending-message-2" }, { scope: "thread" }),
      ],
      call: async (handle) => {
        handle.rekey("pending-agent-1", "ag-2", agent("ag-2"));
        throw new Error("post refused");
      },
      failureSummary: "Could not start the agent",
    });

    expect(result).toBe(false);
    expect(pendingIn("agents").map((record) => record.key)).toEqual(["ag-2"]);
    expect(pendingIn("thread")).toEqual([]);
    expect(notifyError).toHaveBeenCalledTimes(1);
  });

  it("takes a record out when the answer cannot name it", async () => {
    await runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-message-1", agent("pending-message-1"))],
      call: async (handle) => handle.drop("pending-message-1"),
      failureSummary: "Could not start the agent",
    });

    expect(pendingIn("agents")).toEqual([]);
    expect(projectOptimistic("agents", [agent("a")], { keyOf }).map(keyOf)).toEqual(["a"]);
  });

  it("moves a scope with the identity that named it", async () => {
    await runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-message-1", agent("pending-message-1"), { scope: "thread:pending-agent-1" })],
      call: async (handle) => handle.moveScope("thread:pending-agent-1", "thread:ag-2"),
      failureSummary: "Could not start the agent",
    });

    expect(projectOptimistic("thread:pending-agent-1", [], { keyOf })).toEqual([]);
    expect(projectOptimistic("thread:ag-2", [], { keyOf }).map(keyOf)).toEqual(["pending-message-1"]);
  });

  it("refuses a second press only while the first is in flight", async () => {
    let inFlight = null;
    await runOptimistic({
      scope: "agents",
      records: [removeRecord("a")],
      call: async () => {
        inFlight = isPending("agents", "a");
      },
      failureSummary: "Could not remove the agent",
    });
    expect(inFlight).toBe(true);
    expect(isPending("agents", "a")).toBe(false);
    expect(pendingIn("agents").map((record) => record.key)).toEqual(["a"]);

    reconcileOptimistic("agents", [agent("b")], { keyOf });
    expect(isPending("agents", "a")).toBe(false);

    await runOptimistic({
      scope: "agents",
      records: [removeRecord("a")],
      call: async () => {
        throw new Error("nope");
      },
      failureSummary: "Could not remove the agent",
    });
    expect(isPending("agents", "a")).toBe(false);
  });

  it("lets a settled insert go by, so the key it named can be pressed again", async () => {
    await runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-agent-1", agent("pending-agent-1"))],
      call: async (handle) => handle.rekey("pending-agent-1", "ag-2", agent("ag-2")),
      failureSummary: "Could not start the agent",
    });

    expect(pendingIn("agents").map((record) => record.key)).toEqual(["ag-2"]);
    expect(isPending("agents", "ag-2")).toBe(false);
  });

  it("supersedes a standing patch the new one covers, and leaves the rest alone", async () => {
    const running = runOptimistic({
      scope: "agents",
      records: [patchRecord("a", { model: "opus", effort: "" })],
      call: () => new Promise(() => {}),
      failureSummary: "Could not set the model",
    });
    await runOptimistic({
      scope: "agents",
      records: [patchRecord("a", { model: "opus", effort: "high" })],
      call: async () => {},
      failureSummary: "Could not set the model",
    });

    expect(pendingIn("agents").map((record) => record.fields)).toEqual([{ model: "opus", effort: "high" }]);
    expect(projectOptimistic("agents", [agent("a")], { keyOf })).toEqual([
      { id: "a", ordinal: 1, model: "opus", effort: "high" },
    ]);

    await runOptimistic({
      scope: "agents",
      records: [patchRecord("a", { muted: true })],
      call: async () => {},
      failureSummary: "Could not mute",
    });
    expect(pendingIn("agents").map((record) => record.fields)).toEqual([
      { model: "opus", effort: "high" },
      { muted: true },
    ]);
    expect(running).toBeInstanceOf(Promise);
  });

  it("takes the standing records for a key out with the removal of it", async () => {
    await runOptimistic({
      scope: "agents",
      records: [insertRecord("pending-agent-1", agent("pending-agent-1"))],
      call: async (handle) => handle.rekey("pending-agent-1", "ag-2", agent("ag-2")),
      failureSummary: "Could not start the agent",
    });
    await runOptimistic({
      scope: "agents",
      records: [removeRecord("ag-2")],
      call: async () => {},
      failureSummary: "Could not remove the agent",
    });

    expect(pendingIn("agents").map((record) => record.kind)).toEqual(["remove"]);
    expect(projectOptimistic("agents", [], { keyOf })).toEqual([]);
  });

  it("keeps one scope's records out of another's", async () => {
    await runOptimistic({
      scope: "inbox",
      records: [removeRecord("branch:p1:build/login")],
      call: async () => {},
      failureSummary: "Could not finish the branch",
    });

    expect(pendingIn("inbox")).toHaveLength(1);
    expect(pendingIn("agents:branch:p1:build/login")).toEqual([]);
    expect(projectOptimistic("agents:branch:p1:build/login", [agent("a")], { keyOf }).map(keyOf)).toEqual(["a"]);

    expect(provisionalKey("agent")).toBe("pending-agent-1");
    expect(provisionalKey("message")).toBe("pending-message-2");
    expect(isProvisionalKey("pending-agent-1")).toBe(true);
    expect(isProvisionalKey("ag-2")).toBe(false);
    expect(isProvisionalKey(null)).toBe(false);

    resetOptimistic();
    expect(pendingIn("inbox")).toEqual([]);
    expect(provisionalKey("agent")).toBe("pending-agent-1");
  });
});
