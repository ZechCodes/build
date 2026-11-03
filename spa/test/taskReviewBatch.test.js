import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { createTaskReviewBatch } from "../src/core/taskReviewBatch.js";

const anchor = { snapshot_id: "snapshot", directory_id: "dir:one", path: "src/a.js", side: "new", line: 3 };
const defaults = { deviceId: "D", projectId: "P", taskId: "T", taskNumber: 970, snapshot: { id: "snapshot", number: 2 } };
const create = (options = {}) => createTaskReviewBatch({ ...defaults, ...options });
const settle = async () => { for (let n = 0; n < 40; n++) await new Promise(resolve => setTimeout(resolve, 0)); };

beforeEach(() => { globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange; });

describe("review batch drafts", () => {
  it("durably edits and removes drafts and restores them on the same revision only", async () => {
    const model = create({ taskId: "restore" });
    await model.ready;
    const id = await model.add({ body: "Check this", anchor });
    await model.edit(id, { body: "Check this carefully" });
    const other = await model.add({ body: "Remove", anchor: { ...anchor, line: 5 } });
    await model.remove(other);
    await model.summary("My summary");
    model.dispose();
    const restored = create({ taskId: "restore" });
    await restored.ready;
    expect(restored.read()).toMatchObject({ drafts: [{ id, body: "Check this carefully", anchor }], summary: "My summary" });
    const newer = create({ taskId: "restore", snapshot: { id: "new", number: 3 } });
    await newer.ready;
    expect(newer.read().drafts).toEqual([]);
    restored.dispose(); newer.dispose();
  });

  it.each(["request_changes", "approve", ""])("posts ordered drafts then %s with opening line links", async (verdict) => {
    const callRpc = vi.fn(async () => ({}));
    const model = create({ taskId: `order-${verdict}`, callRpc });
    await model.ready;
    await model.add({ body: "First", anchor });
    await model.add({ body: "Commit", anchor: { ...anchor, commit: "a".repeat(40), line: 7, side: "old" }, replyTo: "parent" });
    await model.summary("Summary");
    await model.submit(verdict);
    expect(callRpc.mock.calls.slice(0, 2).map(([, params]) => params)).toEqual([
      { task_id: `order-${verdict}`, body: "First", anchor },
      { task_id: `order-${verdict}`, body: "Commit", anchor: { ...anchor, commit: "a".repeat(40), line: 7, side: "old" }, reply_to: "parent" },
    ]);
    const final = callRpc.mock.calls.at(-1)[1];
    if (verdict) {
      expect(final.opinion).toEqual({ snapshot_id: "snapshot", verdict });
      expect(final.body).toContain("Summary");
      expect(final.body).toContain("[[#970:r2:all:dir:one:src/a.js#L3]]");
      expect(final.body).toContain(`[[#970:r2:commit/${"a".repeat(40)}:dir:one:src/a.js#old-L7]]`);
    } else expect(callRpc.mock.calls.every(([, params]) => !params.opinion)).toBe(true);
    expect(model.read().drafts).toEqual([]);
    model.dispose();
  });

  it("includes earlier Comment anchors in a later verdict without reposting their comments", async () => {
    const postedAnchors = [];
    const callRpc = vi.fn(async () => ({}));
    const model = create({ taskId: "comment-then-verdict", callRpc, priorAnchors: () => postedAnchors });
    await model.ready;
    await model.add({ body: "First round note", anchor });
    await model.submit();
    postedAnchors.push(anchor, { ...anchor });
    await model.submit("request_changes");
    expect(callRpc).toHaveBeenCalledTimes(2);
    expect(callRpc.mock.calls[0][1]).toMatchObject({ body: "First round note", anchor });
    const verdict = callRpc.mock.calls[1][1];
    expect(verdict.anchor).toBeUndefined();
    expect(verdict.opinion.verdict).toBe("request_changes");
    expect(verdict.body.match(/#L3/g)).toHaveLength(1);
    expect(verdict.body).toContain("[[#970:r2:all:dir:one:src/a.js#L3]]");
    model.dispose();
  });

  it("shows an enqueue failure and retries the same IDs without clearing drafts or assuming they are absent", async () => {
    const enqueueBatch = vi.fn().mockRejectedValueOnce(new Error("Disk unavailable")).mockResolvedValue([]);
    const service = { enqueueBatch, readOperations: () => [], subscribe: () => () => {} };
    const model = create({ taskId: "enqueue-retry", writeOperations: service });
    await model.ready;
    await model.add({ body: "Retain me", anchor });
    await expect(model.submit("request_changes")).rejects.toThrow("Disk unavailable");
    const captured = enqueueBatch.mock.calls[0][0];
    expect(model.read().submission).toMatchObject({ status: "failed", enqueueError: "Disk unavailable" });
    expect(model.read().error).toBe("Disk unavailable");
    await expect(model.recover()).rejects.toThrow("result is still being checked");
    expect(model.read().drafts[0].body).toBe("Retain me");
    await model.retry();
    expect(enqueueBatch.mock.calls[1][0]).toEqual(captured);
    expect(model.read().submission).toMatchObject({ status: "queued", enqueueError: null });
    expect(model.read().error).toBe("");
    expect(model.read().drafts[0].body).toBe("Retain me");
    model.dispose();
  });

  it("keeps the mount usable when resuming a saved submission cannot enqueue", async () => {
    const enqueueBatch = vi.fn().mockRejectedValue(new Error("Update this bridge"));
    const service = { enqueueBatch, readOperations: () => [], subscribe: () => () => {} };
    const first = create({ taskId: "resume-enqueue-refusal", writeOperations: service });
    await first.ready;
    await first.add({ body: "Still here", anchor });
    await expect(first.submit("approve")).rejects.toThrow("Update this bridge");
    first.dispose();
    const restored = create({ taskId: "resume-enqueue-refusal", writeOperations: service });
    await expect(restored.ready).resolves.toBeUndefined();
    expect(restored.read().submission.enqueueError).toBe("Update this bridge");
    enqueueBatch.mockResolvedValue([]);
    await restored.retry();
    expect(restored.read().submission.status).toBe("queued");
    restored.dispose();
  });

  it("resumes the same prepared operation IDs after a crash before atomic enqueue", async () => {
    const enqueueBatch = vi.fn().mockRejectedValueOnce(new Error("Disk unavailable")).mockResolvedValue([]);
    const service = { enqueueBatch, readOperations: () => [], subscribe: () => () => {} };
    const first = create({ taskId: "prepared-crash", writeOperations: service });
    await first.ready;
    await first.add({ body: "Keep me", anchor });
    await expect(first.submit("approve")).rejects.toThrow("Disk unavailable");
    const captured = enqueueBatch.mock.calls[0][0];
    first.dispose();
    const restored = create({ taskId: "prepared-crash", writeOperations: service });
    await restored.ready;
    expect(enqueueBatch).toHaveBeenCalledTimes(2);
    expect(enqueueBatch.mock.calls[1][0]).toEqual(captured);
    expect(restored.read().drafts[0].body).toBe("Keep me");
    restored.dispose();
  });

  it("retains accepted comments through a later unapplied failure and restores the failed comment after reload", async () => {
    let listener;
    const rows = new Map();
    const service = {
      subscribe: callback => { listener = callback; return () => {}; }, readOperations: () => [...rows.values()],
      enqueueBatch: async inputs => { for (const input of inputs) if (!rows.has(input.operationId)) rows.set(input.operationId, { ...input, state: "accepted", acknowledged: true, transportAttempted: true }); },
      cancelUnsentBatch: vi.fn(),
    };
    const model = create({ taskId: "late-refusal", writeOperations: service });
    await model.ready;
    await model.add({ body: "Keep this until execution settles", anchor });
    await model.submit("approve");
    expect(model.read().submission).not.toBeNull();
    expect(model.read().drafts).toHaveLength(1);
    const [note, verdict] = [...rows.keys()];
    rows.set(note, { ...rows.get(note), state: "failed", effect: "unapplied", error: { message: "Anchor not in this revision" } });
    rows.set(verdict, { ...rows.get(verdict), state: "failed", effect: "unapplied", error: { message: "Dependency failed" } });
    listener([...rows.values()]);
    await settle();
    model.dispose();
    const restored = create({ taskId: "late-refusal", writeOperations: service });
    await restored.ready;
    expect(restored.read().error).toContain("Anchor not in this revision");
    await restored.recover();
    expect(restored.read().drafts[0].body).toBe("Keep this until execution settles");
    restored.dispose();
  });

  it("retains an offline submission until acknowledgements, recovers failures and never resends the acknowledged prefix", async () => {
    let listener;
    const rows = new Map();
    const writeOperations = {
      subscribe: callback => { listener = callback; return () => {}; },
      readOperations: () => [...rows.values()],
      enqueueBatch: vi.fn(async inputs => {
        for (const input of inputs) rows.set(input.operationId, { ...input, acknowledged: false, state: "queued" });
        return inputs.map(input => input.operationId);
      }),
      cancelUnsentBatch: vi.fn(async ids => { for (const id of ids) rows.set(id, { ...rows.get(id), state: "failed", dismissed: true, effect: "unapplied" }); }),
    };
    const model = create({ taskId: "offline", writeOperations });
    await model.ready;
    const first = await model.add({ body: "First", anchor });
    const second = await model.add({ body: "Second", anchor: { ...anchor, line: 8 } });
    await model.submit("request_changes");
    expect(model.read().submission.status).toBe("queued");
    expect(model.read().drafts).toHaveLength(2);
    const inputs = writeOperations.enqueueBatch.mock.calls[0][0];
    expect(inputs[1].after).toEqual([inputs[0].operationId]);
    expect(inputs[2].after).toEqual([inputs[1].operationId]);
    rows.set(inputs[0].operationId, { ...rows.get(inputs[0].operationId), acknowledged: true, state: "succeeded" });
    rows.set(inputs[1].operationId, { ...rows.get(inputs[1].operationId), acknowledged: true, transportAttempted: true, state: "failed", effect: "unapplied", error: { message: "Line is unavailable" } });
    listener([...rows.values()]);
    await settle();
    expect(model.read().submission.status).toBe("failed");
    expect(model.read().error).toContain("Line is unavailable");
    await model.recover();
    expect(model.read().submission).toBeNull();
    expect(model.read().drafts.map(row => row.id)).toEqual([second]);
    expect(model.read().drafts.map(row => row.id)).not.toContain(first);
    expect(writeOperations.cancelUnsentBatch).toHaveBeenCalledWith([inputs[2].operationId]);
    model.dispose();
  });
});
