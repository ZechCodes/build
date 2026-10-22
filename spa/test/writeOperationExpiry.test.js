import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const statusFixture = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/writes/status.json", import.meta.url)));
const expired = statusFixture.result.operations.find((summary) => summary.state === "result_expired");
const helloFixture = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/writes/epoch-greeting.json", import.meta.url)));
const greeting = { ...helloFixture.result, ack_confirm_methods: ["tasks.comment"] };
const scope = { accountId: "expiry-account", deviceId: "expiry-device", principalId: "expiry-principal" };
const address = { deviceId: scope.deviceId, entityId: "project", kind: "task" };
const request = { method: "tasks.comment", params: { body: "retained draft", task_id: "T" } };
let cache, operations, createService, service;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  operations = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache(scope);
  createService = (await import("../src/core/writeOperations.js")).createWriteOperationService;
});
afterEach(async () => { await service?.close(); service = null; });

function accepted(operationId = expired.operation_id, value = { body: "accepted edit" }) {
  return {
    operationId, request, draft: { body: "retained draft" }, state: "accepted", revision: 1,
    acknowledged: true, journalIncarnation: greeting.journal_incarnation,
    overlayActive: true, overlay: [{ address, kind: "merge", value }],
  };
}
function harness(summary = expired, semantic = null) {
  const session = {
    rawCall: vi.fn(async (method, params, options) => {
      if (method === "tasks.comment") {
        if (semantic) return semantic(method, params, options);
        return {
          ok: true, bridge_epoch: greeting.bridge_epoch,
          write: { phase: "ack", operation_id: options.write.operation_id, revision: 1, state: "accepted", accepted_epoch: greeting.bridge_epoch, owners: [] },
          result: { comment_id: "new-comment" },
        };
      }
      return {
        journal_incarnation: greeting.journal_incarnation, cursor: "1",
        operations: params.operation_ids.includes(summary.operation_id) ? [summary] : [],
      };
    }),
    onWriteFrame: () => () => {},
  };
  service = createService({ ...scope, cache: operations, randomUUID: () => "expiry-tab", setTimeout: () => 1, clearTimeout: () => {} });
  return { session, bind: () => service.bind({ session, greeting }) };
}

describe("expired durable write outcomes", () => {
  it("uses the immutable failed-unapplied tombstone to remove only its own real cache layer", async () => {
    expect(expired).toMatchObject({ outcome: "failed", effect: "unapplied" });
    await cache.writeCached(address, { body: "canonical", title: "canonical" });
    await operations.put(accepted());
    await operations.put(accepted("later-operation", { title: "later accepted edit" }));
    const { session, bind } = harness();
    expect((await cache.readCached(address)).value).toEqual({ body: "accepted edit", title: "later accepted edit" });
    await bind();
    expect(await operations.get(expired.operation_id)).toMatchObject({
      state: "result_expired", revision: expired.revision, outcome: "failed", effect: "unapplied",
      overlayActive: false, unresolved: false, draft: { body: "retained draft" },
    });
    expect((await cache.readCached(address)).value).toEqual({ body: "canonical", title: "later accepted edit" });
    expect(await operations.get("later-operation")).toMatchObject({ state: "accepted", overlayActive: true });
    await expect(service.submit({ ...request, operationId: expired.operation_id })).rejects.toMatchObject({
      error_code: "result_expired", effect: "unapplied", uncertain: false,
    });
    await service.checkAgain(expired.operation_id);
    expect(session.rawCall.mock.calls.filter(([method]) => method === "tasks.comment")).toHaveLength(0);
    await expect(service.submit({ ...request, operationId: "new-user-action" })).resolves.toEqual({ comment_id: "new-comment" });
    expect(session.rawCall.mock.calls.filter(([method]) => method === "tasks.comment")).toHaveLength(1);
    expect(await operations.get(expired.operation_id)).toMatchObject({ state: "result_expired", outcome: "failed", effect: "unapplied" });
  });

  it("rejects an awaiting action definitively when its lost ack reconciles to the immutable expired failure", async () => {
    const { session, bind } = harness(expired, async () => { throw Object.assign(new Error("ack lost"), { timedOut: true, uncertain: true }); });
    await bind();
    await expect(service.submit({ ...request, operationId: expired.operation_id, draft: { body: "retained draft" } })).rejects.toMatchObject({
      error_code: "result_expired", effect: "unapplied", uncertain: false,
    });
    expect(await operations.get(expired.operation_id)).toMatchObject({
      state: "result_expired", outcome: "failed", effect: "unapplied", unresolved: false, draft: { body: "retained draft" },
    });
    await service.checkAgain(expired.operation_id);
    expect(session.rawCall.mock.calls.filter(([method]) => method === "tasks.comment")).toHaveLength(1);
  });

  it.each([
    { outcome: "succeeded", effect: undefined },
    { outcome: "failed", effect: "partial" },
    { outcome: "failed", effect: "unknown" },
  ])("keeps an expired $outcome/$effect result unresolved without a complete result", async (outcome) => {
    await cache.writeCached(address, { body: "canonical" });
    await operations.put(accepted());
    const { bind } = harness({ ...expired, ...outcome });
    await bind();
    expect(await operations.get(expired.operation_id)).toMatchObject({ state: "result_expired", ...outcome, unresolved: true, overlayActive: true });
    expect((await cache.readCached(address)).value.body).toBe("accepted edit");
    await expect(service.submit({ ...request, operationId: expired.operation_id })).rejects.toMatchObject({ error_code: "result_expired", uncertain: true });
  });
});
