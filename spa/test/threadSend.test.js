import { describe, expect, it } from "vitest";
import { createOptimisticStore, insertRecord } from "../src/core/optimistic.js";
import { provisionalThreadItem, threadItemKey } from "../src/core/thread.js";
import { creationMessageHasArrived, provisionalMessageEntry, rekeyPostedMessage } from "../src/core/threadSend.js";

const echo = (sequence, operationId) => ({
  type: "message",
  data: { sequence, role: "user", body: "create this agent", ...(operationId ? { operation_id: operationId } : {}) },
});

const creation = async ({ receipt = null, held = [] } = {}) => {
  const store = createOptimisticStore();
  const provisional = provisionalMessageEntry("pending-message", { body: "create this agent" }, "creation-op");
  let complete;
  const running = store.runOptimistic({
    scope: "thread",
    records: [insertRecord("pending-message", provisional, { clearedBy: creationMessageHasArrived })],
    call: async (handle) => {
      if (receipt) rekeyPostedMessage(handle, "pending-message", provisional, receipt, {});
      await new Promise((resolve) => { complete = resolve; });
    },
  });
  const projected = store.projectOptimistic("thread", held, { keyOf: threadItemKey });
  complete();
  await running;
  store.reconcileOptimistic("thread", held, { keyOf: threadItemKey });
  return { store, projected };
};

describe("the first message projected while an agent is created", () => {
  it("draws an early operation echo once before its receipt arrives", async () => {
    const held = [echo(7, "creation-op")];
    const { store, projected } = await creation({ held });

    expect(projected).toEqual(held);
    expect(store.pendingIn("thread")).toEqual([]);
  });

  it("retires a receipted overlay when the cache already holds its operation stand-in", async () => {
    const held = [provisionalThreadItem({ operationId: "creation-op", message: { body: "create this agent" }, sequence: 7 })];
    const { store, projected } = await creation({ receipt: { posted_sequence: 7 }, held });

    expect(projected).toEqual(held);
    expect(store.pendingIn("thread")).toEqual([]);
  });

  it("matches a legacy echo by receipt sequence while preserving another send", async () => {
    const held = [echo(7), provisionalThreadItem({ operationId: "next-op", message: { body: "send this when ready" }, sequence: 8 })];
    const { store, projected } = await creation({ receipt: { posted_sequence: 7 }, held });

    expect(projected).toEqual(held);
    expect(store.pendingIn("thread")).toEqual([]);
  });
});
