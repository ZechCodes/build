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

  it("keeps review anchors and opinions in accepted and confirmed comments", async () => {
    const anchor = { snapshot_id: "snap", directory_id: "dir", path: "src/a.js", line: 2, side: "new" };
    const opinion = { snapshot_id: "snap", verdict: "request_changes" };
    const record = operation("tasks.comment", { task_id: ack.result.task_id });
    const frame = { ...ack, result: { ...ack.result, overlay: [{ ...ack.result.overlay[0], anchor, opinion }] } };
    expect((await effects.normalizeAckEffects(frame, record)).overlay[0].value).toMatchObject({ anchor, opinion });
    const confirmed = { ...success, result: { ...success.result, comment: { ...success.result.comment, anchor, opinion } } };
    expect((await effects.normalizeAckEffects(confirmed, record)).overlay[0].value).toMatchObject({ anchor, opinion });
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
    const canonical = { task: { watched: false, ...success.result.task }, timeline: [{ ...success.result.comment, type: "comment" }] };
    const { refreshWriteOperation } = await import("../src/core/writeOperationRefresh.js");
    const frame = { ...success, refresh: [{ kind: "task", id: canonical.task.id }] };
    await operations.update(record.operationId, (row) => ({ ...row, confirmation: frame, refreshPending: true }));
    const { TASK_READ_COUNT_ADDRESS } = await import("../src/core/taskReadOrder.js");
    let issued;
    expect(await refreshWriteOperation({ cache: operations, frame, record: await operations.get(record.operationId),
      authority: { active: () => true, call: vi.fn(async () => {
        issued = (await cache.readCanonicalCached(TASK_READ_COUNT_ADDRESS)).value;
        return canonical;
      }) } })).toMatchObject({ complete: true });
    const painted = (await cache.readCached(address)).value;
    const proof = painted.task.__task_content_cache;
    expect(Object.fromEntries(Object.entries(painted.task).filter(([key]) => !key.startsWith("__")))).toEqual(canonical.task);
    expect(painted.timeline).toEqual(canonical.timeline);
    expect(proof.header.read_order).toBe(issued);
    expect(proof.body.read_order).toBe(issued);
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

const projectionAddress = (params) => threadCacheAddress({ deviceId: scope.deviceId, entityId: params.entity_id,
  agentId: params.agent_id, conversationId: params.conversation_id, threadId: params.thread_id });

const projectionWindow = (params) => ({ thread_id: params.thread_id, thread_generation_revision: 0,
  items: [{ type: "message", data: { id: `${params.thread_id}-canonical`, sequence: 1, body: "canonical" } }],
  deliveredSequence: 1, knownTotalItems: 1 });

async function acceptProjectionThread(owner, id, params, body, sequence, identity = scope) {
  const record = operation("thread.post", params, { ...identity, operationId: id, draft: { body, attachments: [] } });
  const frame = { ...ack, write: { ...ack.write, operation_id: id,
    owners: ack.write.owners.map((owner) => ({ ...owner, sequence })) },
  result: { message_id: `${id}-message`, accepted_at: "2026-10-21T15:00:00Z", overlay: [{
    kind: "thread_message", message_id: `${id}-message`, body, sequence, created_at: "2026-10-21T15:00:01Z",
  }] } };
  await owner.put(record);
  const normalized = await effects.normalizeAckEffects(frame, record);
  expect(normalized.overlay).toHaveLength(1);
  await owner.accept(id, { ...normalized, revision: 1, ack: frame });
  return record;
}

describe("projection composition preservation", () => {
  it("preserves the original grouped read order and ordered accepted layers through one unapplied retirement", async () => {
    const first = { ...threadParams, conversation_id: "projection-first-conversation", thread_id: "projection-first" };
    const second = { ...threadParams, conversation_id: "projection-second-conversation", thread_id: "projection-second" };
    const absent = projectionAddress({ ...threadParams, conversation_id: "projection-absent-conversation", thread_id: "projection-absent" });
    const firstAddress = projectionAddress(first), secondAddress = projectionAddress(second);
    await cache.writeCached(firstAddress, projectionWindow(first));
    await cache.writeCached(secondAddress, projectionWindow(second));
    const group = [absent, firstAddress, secondAddress, firstAddress];
    const canonical = await cache.readCanonicalCachedMany(group);
    // Accept the higher owner sequence first: layer order must retain the
    // source's owner sequence rather than its local arrival order.
    await acceptProjectionThread(operations, "projection-high", first, "higher sequence", 3);
    const lower = await acceptProjectionThread(operations, "projection-low", first, "lower sequence", 2);
    await acceptProjectionThread(operations, "projection-other", second, "other thread", 2);
    const projected = await cache.readCachedMany(group);
    expect(projected).toHaveLength(group.length);
    expect(projected[0]).toBeUndefined();
    expect(projected[1].value.items.map(({ data }) => data.body)).toEqual(["canonical", "lower sequence", "higher sequence"]);
    expect(projected[2].value.items.map(({ data }) => data.body)).toEqual(["canonical", "other thread"]);
    expect(projected[3]).toEqual(projected[1]);
    expect(projected.slice(1).every(({ accepted }) => accepted === true)).toBe(true);
    expect(await cache.readCanonicalCachedMany(group)).toEqual(canonical);
    await operations.confirm(lower.operationId, { revision: 2, confirmation: { type: "writes.confirmed",
      operation_id: lower.operationId, revision: 2, state: "failed", effect: "unapplied", published: [] } });
    const after = await cache.readCachedMany(group);
    expect(after[0]).toBeUndefined();
    expect(after[1].value.items.map(({ data }) => data.body)).toEqual(["canonical", "higher sequence"]);
    expect(after[2]).toEqual(projected[2]);
    expect(after[3]).toEqual(after[1]);
    expect((await operations.get(lower.operationId)).overlayActive).toBe(false);
    expect((await operations.get("projection-high")).overlayActive).toBe(true);
    expect((await operations.get("projection-other")).overlayActive).toBe(true);
    expect(await cache.readCanonicalCachedMany(group)).toEqual(canonical);
  });

  it("keeps replacement activation visible when the prior same-namespace owner is disposed", async () => {
    const address = projectionAddress(threadParams);
    await cache.writeCached(address, projectionWindow(threadParams));
    const canonical = await cache.readCanonicalCached(address);
    const record = await acceptProjectionThread(operations, "projection-owned", threadParams, "accepted owner", 2);
    const { createWriteOperationCache } = await import("../src/core/writeOperationCache.js");
    const replacement = createWriteOperationCache(scope);
    replacement.activateProjection();
    operations.deactivateProjection();
    expect((await cache.readCached(address)).value.items.map(({ data }) => data.body)).toEqual(["canonical", "accepted owner"]);
    replacement.deactivateProjection();
    expect(await cache.readCached(address)).toEqual(canonical);
    expect((await replacement.get(record.operationId)).overlayActive).toBe(true);
    replacement.activateProjection();
    operations.deactivateProjection();
    expect((await cache.readCached(address)).accepted).toBe(true);
    expect((await cache.readCached(address)).value.items.map(({ data }) => data.body)).toEqual(["canonical", "accepted owner"]);
    expect(await cache.readCanonicalCached(address)).toEqual(canonical);
  });

  it("switches the exact active namespace and clears its layers through the real pending-operation reset", async () => {
    const address = projectionAddress(threadParams);
    await cache.writeCached(address, projectionWindow(threadParams));
    const canonical = await cache.readCanonicalCached(address);
    await acceptProjectionThread(operations, "projection-primary", threadParams, "primary accepted", 2);
    const { createWriteOperationCache, wipePendingOperations } = await import("../src/core/writeOperationCache.js");
    const foreignScope = { ...scope, accountId: "foreign-projection-account" };
    const foreign = createWriteOperationCache(foreignScope);
    await acceptProjectionThread(foreign, "projection-foreign", threadParams, "foreign accepted", 2, foreignScope);
    expect((await cache.readCached(address)).value.items.map(({ data }) => data.body)).toEqual(["canonical", "primary accepted"]);
    foreign.activateProjection();
    operations.deactivateProjection();
    expect((await cache.readCached(address)).value.items.map(({ data }) => data.body)).toEqual(["canonical", "foreign accepted"]);
    await wipePendingOperations();
    expect(await cache.readCached(address)).toEqual(canonical);
    expect(await operations.list()).toEqual([]);
    expect(await foreign.list()).toEqual([]);
    // A still-held factory can persist a new accepted row, but the account
    // reset must not let that write implicitly reactivate its prior scope.
    await acceptProjectionThread(foreign, "projection-after-reset", threadParams, "accepted after reset", 3, foreignScope);
    expect(await cache.readCached(address)).toEqual(canonical);
    operations.activateProjection();
    expect(await cache.readCached(address)).toEqual(canonical);
    foreign.activateProjection();
    expect((await cache.readCached(address)).value.items.map(({ data }) => data.body)).toEqual(["canonical", "accepted after reset"]);
    foreign.deactivateProjection();
    expect(await cache.readCached(address)).toEqual(canonical);
    expect(await cache.readCanonicalCached(address)).toEqual(canonical);
  });
});
