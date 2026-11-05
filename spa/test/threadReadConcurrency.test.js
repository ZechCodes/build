import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const scope = { accountId: "account-concurrency", deviceId: "device-concurrency", principalId: "principal-concurrency" };
const address = { deviceId: scope.deviceId, entityId: "workspace-concurrency", kind: "thread", sub: "conversation-concurrency" };
const message = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });
let cache, conversation, syncThreadWindow, operations;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  conversation = await import("../src/core/conversationCache.js");
  ({ syncThreadWindow } = await import("../src/core/threadSync.js"));
  operations = null;
});
afterEach(() => operations?.deactivateProjection());

const savedThread = async () => (await cache.readCanonicalCached(address))?.value;

describe("conversation reads alongside real local writes", () => {
  it("preserves a provisional send and its acknowledgement target when a cold page lands", async () => {
    let releasePage, reportStarted;
    const started = new Promise((resolve) => { reportStarted = resolve; });
    const page = new Promise((resolve) => { releasePage = resolve; });
    const call = vi.fn(() => { reportStarted(); return page; });
    const syncing = syncThreadWindow({ deviceId: scope.deviceId, entityId: address.entityId,
      agentId: "agent-concurrency", conversationId: address.sub, active: () => true, call });
    const incoming = { thread_id: "thread-concurrency", items: [message(1, "Existing bridge reply")], has_more: false, thread_last_sequence: 1 };
    try {
      await started;
      expect(await savedThread()).toBeUndefined();
      await conversation.writeProvisionalMessage(address, "operation-inflight-send", { body: "Sent while the cold read waited", attachments: [] });
      expect((await savedThread()).items).toEqual([expect.objectContaining({ data: expect.objectContaining({
        operation_id: "operation-inflight-send", provisional: true, body: "Sent while the cold read waited",
      }) })]);
      releasePage(incoming);
      expect(await syncing).toBe(true);
      const beforeAck = (await savedThread()).items.find(({ data }) => data.operation_id === "operation-inflight-send");
      await conversation.acknowledgeProvisionalMessage(address, "operation-inflight-send", 2, "sent");
      const stored = await savedThread();
      const acknowledged = stored.items.find(({ data }) => data.operation_id === "operation-inflight-send");
      expect(beforeAck).toMatchObject({ data: { provisional: true, sequence: null, body: "Sent while the cold read waited" } });
      expect(acknowledged).toMatchObject({ data: { operation_id: "operation-inflight-send", provisional: true,
        sequence: 2, delivery_status: "sent", body: "Sent while the cold read waited" } });
      expect(stored.items.filter(({ data }) => data.operation_id === "operation-inflight-send")).toHaveLength(1);
      expect(stored.items).toContainEqual(incoming.items[0]);
    } finally {
      releasePage(incoming);
      await syncing;
    }
  });

  it("completes a real required write-operation refresh over an ID-less cached conversation", async () => {
    const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
    const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
    const { provisionalThreadItem } = await import("../src/core/thread.js");
    operations = createWriteOperationCache(scope);
    operations.activateProjection();
    await cache.writeCached(address, { items: [message(1, "Cached before confirmation")], deliveredSequence: 1, activityDigests: [] });
    const operationId = "operation-required-thread-refresh";
    const pending = provisionalThreadItem({ operationId, message: { body: "Accepted post awaiting canonical refresh" } });
    const publication = [{ kind: "thread", id: address.sub, generation: 0, sequence: 7 }];
    await operations.put({ operationId, revision: 0,
      request: { method: "thread.post", params: { entity_id: address.entityId, agent_id: "agent-concurrency",
        conversation_id: address.sub, body: pending.data.body } },
      owner: { kind: "chat", entityId: address.entityId, agentId: "agent-concurrency", conversationId: address.sub },
      overlay: [{ address, kind: "upsert", path: ["items"], key: ["data", "operation_id"], insertOnly: true, value: pending }],
    });
    await operations.accept(operationId, { ack: {}, revision: 1, owners: publication });
    const frame = { operation_id: operationId, revision: 2, state: "succeeded", effect: "applied", published: publication,
      refresh: [{ kind: "thread", id: address.sub }] };
    const record = await operations.confirm(operationId, { revision: 2, confirmation: frame, refreshPending: true });
    expect(record.overlayActive).toBe(true);
    expect((await savedThread()).thread_id).toBeUndefined();
    const arrived = message(2, "Canonical post is now visible");
    const authority = { active: () => true, call: vi.fn(async () => ({ thread_id: "thread-confirmed",
      items: [arrived], has_more: false, thread_last_sequence: 2 })) };
    await expect(refreshWriteOperation({ cache: operations, frame, record, authority })).resolves.toBeUndefined();
    expect(authority.call).toHaveBeenCalledWith("thread.page", { entity_id: address.entityId, agent_id: "agent-concurrency",
      conversation_id: address.sub, newest: true, limit: 100 });
    const canonical = await cache.readCanonicalCached(address);
    expect(canonical.value.items).toContainEqual(arrived);
    expect(canonical.publication).toEqual(publication);
    expect((await operations.get(operationId)).overlayActive).toBe(false);
    expect((await cache.readCached(address)).value.items.some(({ data }) => data.operation_id === operationId)).toBe(false);
  });
});
