import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const ack = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/writes/ack.json", import.meta.url)));
const success = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/writes/confirmed-success.json", import.meta.url)));
const taskFixture = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/tasks.get.json", import.meta.url))).result;
const scope = { accountId: "account", deviceId: "device", principalId: "principal" };
const operation = (method, params, extra = {}) => ({
  ...scope, operationId: ack.write.operation_id, request: { method, params }, method,
  createdAt: Date.parse("2026-10-21T14:59:00Z"), ...extra,
});
let effects, cache, operations, taskAddress, threadCacheAddress;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  effects = await import("../src/core/writeOperationEffects.js");
  cache = await import("../src/core/localCache.js");
  ({ taskAddress } = await import("../src/core/trackerCache.js"));
  ({ threadCacheAddress } = await import("../src/core/conversationCache.js"));
  operations = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache(scope);
  operations.activateProjection();
});

async function accept(record, frame = ack) {
  await operations.put(record);
  const normalized = await effects.normalizeAckEffects(frame, record);
  await operations.accept(record.operationId, { ...normalized, revision: 1, ack: frame });
  return normalized;
}

const threadParams = { entity_id: "workspace", agent_id: "agent", conversation_id: "conversation", thread_id: "thread", body: " draft ", attachments: [] };
const threadRecord = () => operation("thread.post", threadParams, { draft: { body: " draft ", attachments: [] } });
const threadAddress = () => threadCacheAddress({ deviceId: scope.deviceId, entityId: "workspace", agentId: "agent", conversationId: "conversation", threadId: "thread" });

describe("typed durable write effects", () => {
  it("normalizes the actual ack fixture into a task timeline without replacing its task", async () => {
    const address = taskAddress(scope.deviceId, "proj-1", ack.result.task_id);
    const canonical = { ...taskFixture, timeline: taskFixture.timeline.filter(row => row.id !== ack.result.comment_id) };
    await cache.writeCached(address, canonical);
    const record = operation("tasks.comment", { task_id: ack.result.task_id, body: "local draft" });
    const normalized = await accept(record);
    expect(normalized.reservedIds).toMatchObject({ task_id: ack.result.task_id, comment_id: ack.result.comment_id });
    expect(normalized.overlay).toEqual([expect.objectContaining({ address, kind: "upsert", path: ["timeline"], key: "id", value: expect.objectContaining({
      type: "comment", id: ack.result.comment_id, task_id: ack.result.task_id, body: ack.result.overlay[0].body,
      author: { kind: "user" }, created_at: ack.result.accepted_at,
    }) })]);
    expect((await cache.readCached(address)).value.task).toEqual(canonical.task);
    expect((await cache.readCached(address)).value.timeline).toHaveLength(canonical.timeline.length + 1);
    expect((await cache.readCanonicalCached(address)).value).toEqual(canonical);
  });

  it("resolves task projects from request parameters and held task addresses", async () => {
    const noOwners = { ...ack, write: { ...ack.write, owners: [] } };
    const params = { task_id: ack.result.task_id, project_id: "explicit" };
    const explicit = await effects.normalizeAckEffects(noOwners, operation("tasks.comment", params));
    expect(explicit.overlay[0].address.entityId).toBe("explicit");
    await cache.writeCached(taskAddress(scope.deviceId, "held-project", ack.result.task_id), taskFixture);
    const held = await effects.normalizeAckEffects(noOwners, operation("tasks.comment", { task_id: ack.result.task_id }));
    expect(held.overlay[0].address.entityId).toBe("held-project");
  });

  it("paints the successful confirmation fixture before ack until its canonical timeline arrives", async () => {
    const record = operation("tasks.comment", { task_id: success.result.task.id, body: "local draft" });
    await operations.put(record);
    const normalized = await effects.normalizeAckEffects(success, record);
    const address = taskAddress(scope.deviceId, success.result.task.project_id, success.result.task.id);
    expect(normalized.overlay).toEqual([expect.objectContaining({ address, value: { ...success.result.comment, type: "comment" } })]);
    expect(normalized.reservedIds).toMatchObject({ task_id: success.result.task.id, comment_id: success.result.comment.id });
    await operations.confirm(record.operationId, { ...normalized, confirmation: success, revision: success.revision });
    expect((await cache.readCached(address)).value.timeline).toEqual([{ ...success.result.comment, type: "comment" }]);
    const canonical = { task: success.result.task, timeline: [{ ...success.result.comment, type: "comment" }] };
    await operations.observePublication({ canonical: [{ address, value: canonical, publication: success.published }] });
    expect((await cache.readCached(address)).value).toEqual(canonical);
    expect((await operations.get(record.operationId)).overlayActive).toBe(false);
  });

  it("uses the full confirmation task's project when publication ownership is absent", async () => {
    const record = operation("tasks.comment", { task_id: success.result.task.id });
    const normalized = await effects.normalizeAckEffects({ ...success, published: [] }, record);
    expect(normalized.overlay[0].address.entityId).toBe(success.result.task.project_id);
  });

  it("ignores unknown wire effects even when they contain executable cache descriptors", async () => {
    const malicious = { kind: "merge", address: { deviceId: "other", entityId: "account", kind: "settings" }, path: ["__proto__"], value: { compromised: true } };
    const frame = { ...ack, result: { overlay: [malicious, { kind: "future_effect", effects: [malicious] }] } };
    const normalized = await effects.normalizeAckEffects(frame, operation("tasks.comment", { task_id: "task" }));
    expect(normalized.overlay).toEqual([]);
    expect(frame.result.overlay).toEqual([malicious, { kind: "future_effect", effects: [malicious] }]);
    expect({}.compromised).toBeUndefined();
  });

  it("paints one persisted thread draft while retaining the canonical window", async () => {
    const record = threadRecord();
    const address = threadAddress();
    const canonical = { items: [{ type: "message", data: { sequence: 1, body: "earlier" } }], deliveredSequence: 1, knownTotalItems: 1 };
    await cache.writeCached(address, canonical);
    const normalized = await effects.normalizeIntentEffects(record);
    await operations.put({ ...record, ...normalized, overlayActive: true });
    const painted = (await cache.readCached(address)).value;
    expect(painted.deliveredSequence).toBe(1);
    expect(painted.items).toHaveLength(2);
    expect(painted.items[1].data).toMatchObject({ operation_id: record.operationId, body: " draft ", provisional: true, delivery_status: "queued", created_at: "2026-10-21T14:59:00.000Z" });
    expect((await cache.readCanonicalCached(address)).value).toEqual(canonical);
  });

  it("uses accepted thread text, timestamp and reserved identity, and lets its canonical echo win", async () => {
    const record = threadRecord();
    const frame = { ...ack, result: { message_id: "message-1", accepted_at: "2026-10-21T15:00:00Z", overlay: [{
      kind: "thread_message", message_id: "message-1", body: "normalized", created_at: "2026-10-21T15:00:01Z", sequence: 2,
    }] } };
    const normalized = await accept(record, frame);
    expect(normalized.reservedIds.message_id).toBe("message-1");
    const layer = normalized.overlay[0];
    expect(layer).toMatchObject({ kind: "upsert", path: ["items"], key: ["data", "operation_id"], insertOnly: true });
    expect(layer.value.data).toMatchObject({ body: "normalized", created_at: "2026-10-21T15:00:01Z", id: "message-1", sequence: 2, delivery_status: "sent" });
    const echo = { type: "message", data: { operation_id: record.operationId, sequence: 2, body: "canonical", provisional: false } };
    await cache.writeCached(threadAddress(), { items: [echo], deliveredSequence: 2 });
    expect((await cache.readCached(threadAddress())).value.items).toEqual([echo]);
  });

  it("retains the local thread draft when no typed normalization is supplied", async () => {
    const record = threadRecord();
    const normalized = await effects.normalizeAckEffects({ ...ack, result: {} }, record);
    expect(normalized.overlay[0].value.data.body).toBe(record.draft.body);
    const unknown = await effects.normalizeIntentEffects(operation("future.write", {}, { draft: { body: "untrusted replacement" } }));
    expect(unknown.overlay).toEqual([]);
  });
});
