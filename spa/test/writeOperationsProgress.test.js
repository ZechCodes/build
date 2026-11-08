import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const identity = { accountId: "account", deviceId: "device", principalId: "principal" };
const greeting = { bridge_epoch: "epoch", journal_incarnation: "journal", capabilities: ["bridge.epoch", "environments"],
  ack_confirm_methods: ["environment.create_project", "workspace.create"] };
const progress = [{ step: "project", state: "done" }, { step: "workspace", state: "running", detail: "Copying app" }];
const frame = (extra = {}) => ({ type: "writes.progress", bridge_epoch: "epoch", operation_id: "attempt", progress, ...extra });
const intent = (extra = {}) => ({ operationId: "attempt", request: { method: "environment.create_project", params: { environment_id: "env", name: "Work", workspace: { name: "Work" } } },
  revision: 1, state: "accepted", acknowledged: true, journalIncarnation: "journal", ...extra });
let cache, service, factory, makeService;
const resources = [];
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  factory = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache;
  makeService = (await import("../src/core/writeOperations.js")).createWriteOperationService;
  cache = factory(identity);
});
afterEach(async () => { for (const held of resources.splice(0)) await held.close(); });
async function bind({ currentGreeting = greeting, active = () => true, status = [], raw = () => new Promise(() => {}) } = {}) {
  const session = { callRaw: vi.fn(async (method, ...args) => method.startsWith("writes.")
    ? { journal_incarnation: "journal", operations: status } : raw(method, ...args)), onWriteFrame: () => () => {} };
  service = makeService({ ...identity, cache, randomUUID: () => "attempt", setTimeout: () => 1, clearTimeout() {} });
  resources.push(service);
  await service.bind({ session, greeting: currentGreeting, active });
  return session;
}

describe("creation write progress", () => {
  it("records Checking after the sending opening disconnects before its reply rejects", async () => {
    let active = true, rejectSend;
    await bind({ active: () => active, raw: () => new Promise((_resolve, reject) => { rejectSend = reject; }) });
    void service.submit({ operationId: "attempt", ...intent().request }).catch(() => {});
    await vi.waitFor(() => expect(rejectSend).toBeTypeOf("function"));
    active = false;
    rejectSend(Object.assign(new Error("Disconnected"), { uncertain: true }));
    await vi.waitFor(async () => expect(await cache.get("attempt")).toMatchObject({ checking: true, state: "queued", acknowledged: false }));
    expect(await cache.get("attempt")).not.toHaveProperty("unresolved");
    expect(await cache.list()).toHaveLength(1);
  });

  it.each(["ack", "progress", "status", "terminal"])("does not let an old send rejection overwrite newer %s evidence", async kind => {
    let active = true, rejectSend;
    const session = await bind({ active: () => active, raw: () => new Promise((_resolve, reject) => { rejectSend = reject; }) });
    void service.submit({ operationId: "attempt", ...intent().request }).catch(() => {});
    await vi.waitFor(() => expect(rejectSend).toBeTypeOf("function"));
    active = false;
    await service.bind({ session, greeting, active: () => true });
    if (kind === "ack") await service.handleFrame({ ok: true, bridge_epoch: "epoch", write: { phase: "ack", state: "accepted", operation_id: "attempt", revision: 1 } });
    if (kind === "progress") await service.handleFrame(frame());
    if (kind === "status") {
      session.callRaw.mockImplementation(async () => ({ journal_incarnation: "journal", operations: [{ operation_id: "attempt", state: "executing", revision: 0 }] }));
      await service.reconcile();
    }
    if (kind === "terminal") await service.handleFrame({ type: "writes.confirmed", bridge_epoch: "epoch", operation_id: "attempt", revision: 2, state: "succeeded", result: { workspace: { id: "W" } } });
    const before = await cache.get("attempt");
    const announced = vi.fn(); const stop = service.subscribe(announced);
    rejectSend(Object.assign(new Error("Old opening disconnected"), { uncertain: true }));
    await vi.waitFor(() => expect(announced).toHaveBeenCalled());
    expect(await cache.get("attempt")).toEqual(before);
    expect((await cache.get("attempt")).checking).not.toBe(true);
    stop();
  });
  it("records whether a new creation has reached transport before its raw handoff", async () => {
    const raw = vi.fn(async () => {
      expect(await cache.get("attempt")).toMatchObject({ transportAttempted: true });
      return { ok: true, bridge_epoch: "epoch", write: { phase: "ack", state: "accepted", operation_id: "attempt", revision: 1 } };
    });
    const acquire = vi.spyOn(cache, "acquireLease").mockResolvedValue(false);
    await bind({ raw });
    void service.submit({ operationId: "attempt", ...intent().request }).catch(() => {});
    await vi.waitFor(async () => expect(await cache.get("attempt")).toMatchObject({ transportAttempted: false }));
    expect(raw).not.toHaveBeenCalled();
    acquire.mockRestore();
    await service.reconcile();
    await vi.waitFor(() => expect(raw).toHaveBeenCalledOnce());
  });

  it("makes a provably never-sent creation recoverable after an environments downgrade", async () => {
    await cache.put(intent({ state: "queued", acknowledged: false, revision: 0, transportAttempted: false, draft: { name: "Work" } }));
    const session = await bind({ currentGreeting: { ...greeting, capabilities: ["bridge.epoch"] },
      status: [{ operation_id: "attempt", state: "not_found" }] });
    expect(await cache.get("attempt")).toMatchObject({ state: "failed", effect: "unapplied", acknowledged: true, overlayActive: false,
      draft: { name: "Work" }, confirmation: { state: "failed", effect: "unapplied", error: { code: "write_unsupported" } } });
    expect(session.callRaw.mock.calls.every(([method]) => method.startsWith("writes."))).toBe(true);
  });

  it.each([true, undefined])("keeps a possibly sent creation Checking on downgrade (transport marker %s)", async transportAttempted => {
    await cache.put(intent({ state: "queued", acknowledged: false, revision: 0, ...(transportAttempted === undefined ? {} : { transportAttempted }) }));
    const session = await bind({ currentGreeting: { ...greeting, capabilities: ["bridge.epoch"] },
      status: [{ operation_id: "attempt", state: "not_found" }] });
    expect(await cache.get("attempt")).toMatchObject({ state: "queued", acknowledged: false, checking: true });
    expect(await cache.get("attempt")).not.toHaveProperty("unresolved");
    expect(await cache.get("attempt")).not.toHaveProperty("confirmation");
    expect(session.callRaw.mock.calls.every(([method]) => method.startsWith("writes."))).toBe(true);
    await service.handleFrame({ type: "writes.confirmed", bridge_epoch: "epoch", operation_id: "attempt", state: "succeeded", revision: 2, result: { workspace: { id: "W" } } });
    expect(await cache.get("attempt")).toMatchObject({ state: "succeeded", confirmation: { result: { workspace: { id: "W" } } } });
  });

  it("does not apply creation downgrade recovery to ordinary writes", async () => {
    await cache.put(intent({ request: { method: "tasks.update", params: { task_id: "T" } }, state: "queued", acknowledged: false, revision: 0, transportAttempted: false }));
    await bind({ currentGreeting: { ...greeting, capabilities: ["bridge.epoch"] } });
    expect(await cache.get("attempt")).toMatchObject({ state: "queued", acknowledged: false });
    expect(await cache.get("attempt")).not.toHaveProperty("checking");
  });

  it("keeps an acknowledged creation pollable if the downgraded bridge cannot find it", async () => {
    await cache.put(intent({ transportAttempted: true }));
    await bind({ currentGreeting: { ...greeting, capabilities: ["bridge.epoch"] }, status: [{ operation_id: "attempt", state: "not_found" }] });
    expect(await cache.get("attempt")).toMatchObject({ state: "accepted", acknowledged: true, checking: true });
    expect(await cache.get("attempt")).not.toHaveProperty("unresolved");
    await service.reconcile();
    expect(await cache.get("attempt")).toMatchObject({ state: "accepted", checking: true });
  });
  it("marks a lost creation reply Checking without changing the polling state or retry identity", async () => {
    const status = [];
    const session = await bind({ status, raw: async () => { throw Object.assign(new Error("Reply lost"), { uncertain: true, timedOut: true }); } });
    const request = { operationId: "attempt", ...intent().request };
    void service.submit(request).catch(() => {});
    await vi.waitFor(async () => expect(await cache.get("attempt")).toMatchObject({ checking: true }));
    expect(await cache.get("attempt")).toMatchObject({ state: "queued", acknowledged: false, request: intent().request });
    expect(await cache.get("attempt")).not.toHaveProperty("unresolved");
    status.push({ operation_id: "attempt", state: "not_found" });
    await service.reconcile();
    const attempts = session.callRaw.mock.calls.filter(([method]) => method === "environment.create_project");
    expect(attempts.length).toBeGreaterThan(1);
    expect(attempts.every(([, params, options]) => options.write.operation_id === "attempt" && JSON.stringify(params) === JSON.stringify(intent().request.params))).toBe(true);
    expect(await cache.list()).toHaveLength(1);
  });

  it("clears Checking on executing status without progress, ack, or fresh progress", async () => {
    await cache.put(intent({ checking: true }));
    await bind({ status: [{ operation_id: "attempt", state: "executing", revision: 2 }] });
    expect(await cache.get("attempt")).toMatchObject({ checking: false, state: "executing", revision: 2 });
    await cache.update("attempt", row => ({ ...row, checking: true }));
    await service.handleFrame(frame());
    expect(await cache.get("attempt")).toMatchObject({ checking: false, state: "executing" });
    await cache.update("attempt", row => ({ ...row, checking: true }));
    await service.handleFrame({ ok: true, bridge_epoch: "epoch", write: { phase: "ack", state: "accepted", operation_id: "attempt", revision: 3 } });
    expect(await cache.get("attempt")).toMatchObject({ checking: false, state: "accepted", revision: 3 });
  });

  it("keeps uncertain ordinary writes unchanged", async () => {
    await bind({ currentGreeting: { ...greeting, ack_confirm_methods: [...greeting.ack_confirm_methods, "tasks.update"] },
      raw: async () => { throw Object.assign(new Error("Reply lost"), { uncertain: true }); } });
    void service.submit({ method: "tasks.update", params: { task_id: "T" } }).catch(() => {});
    await vi.waitFor(async () => expect(await cache.get("attempt")).toBeDefined());
    await service.reconcile();
    expect(await cache.get("attempt")).not.toHaveProperty("checking");
  });
  it("persists one exact creation intent and reuses its frame identity on a caller retry", async () => {
    const session = await bind({ raw: async () => ({ ok: true, bridge_epoch: "epoch", result: { accepted: true },
      write: { phase: "ack", state: "accepted", operation_id: "attempt", revision: 1, accepted_epoch: "epoch" } }) });
    const request = { operationId: "attempt", ...intent().request, draft: { name: "Work" } };
    await expect(service.submit(request)).resolves.toEqual({ accepted: true });
    await expect(service.submit(request)).resolves.toEqual({ accepted: true });
    const calls = session.callRaw.mock.calls.filter(([method]) => method === "environment.create_project");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toEqual(["environment.create_project", intent().request.params, expect.objectContaining({ write: { operation_id: "attempt", after: undefined } })]);
    expect(await cache.get("attempt")).toMatchObject({ request: intent().request, draft: { name: "Work" } });
    expect((await cache.list())).toHaveLength(1);
    expect(calls[0][1]).not.toHaveProperty("operation_id");
  });
  it("persists display-only push progress and restores it across reload without changing outcome evidence", async () => {
    await cache.put(intent()); await bind();
    const before = await cache.get("attempt");
    expect(await service.handleFrame(frame())).toBe(true);
    expect(await cache.get("attempt")).toEqual({ ...before, progress, creationEvidence: 1 });
    expect(service.readOperations()[0].progress).toEqual(progress);
    await service.close();
    vi.resetModules();
    const restored = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache(identity);
    expect(await restored.get("attempt")).toMatchObject({ state: "accepted", acknowledged: true, revision: 1, progress });
    expect(await factory({ ...identity, accountId: "other" }).get("attempt")).toBeUndefined();
  });

  it("does not resolve Create even when all progress steps say done; terminal confirmation alone settles it", async () => {
    await bind();
    let settled = false;
    const pending = service.submit({ operationId: "attempt", method: "workspace.create", params: { project_id: "P", name: "Work" } });
    pending.then(() => { settled = true; }, () => {});
    await vi.waitFor(async () => expect(await cache.get("attempt")).toBeDefined());
    await service.handleFrame(frame({ progress: [{ step: "workspace", state: "done" }] }));
    expect(await cache.get("attempt")).toMatchObject({ state: "queued", acknowledged: false, revision: 0 });
    expect(settled).toBe(false);
    const result = { workspace: { id: "workspace", project_id: "P" } };
    await service.handleFrame({ type: "writes.confirmed", bridge_epoch: "epoch", operation_id: "attempt", revision: 2, state: "succeeded", result });
    await expect(pending).resolves.toEqual(result);
    await service.handleFrame(frame({ progress: [{ step: "workspace", state: "failed" }] }));
    expect(await cache.get("attempt")).toMatchObject({ state: "succeeded", progress: [{ step: "workspace", state: "done" }] });
  });

  it("reads optional status progress while keeping the accepted operation pending", async () => {
    await cache.put(intent());
    await bind({ status: [{ operation_id: "attempt", state: "executing", revision: 2, progress }] });
    expect(await cache.get("attempt")).toMatchObject({ state: "executing", revision: 2, progress });
    expect(await cache.get("attempt")).not.toHaveProperty("confirmation");
  });

  it("does not treat progress as success or failure, including failed step progress", async () => {
    await cache.put(intent()); await bind();
    await service.handleFrame(frame({ state: "succeeded", result: { project: "forged" }, progress: [{ step: "workspace", state: "failed", detail: "Refused" }] }));
    expect(await cache.get("attempt")).toMatchObject({ state: "accepted", revision: 1, progress: [{ step: "workspace", state: "failed", detail: "Refused" }] });
    expect(await cache.get("attempt")).not.toHaveProperty("confirmation");
  });

  it("rejects another epoch, journal, unknown id and obsolete opening", async () => {
    await cache.put(intent()); let active = true; await bind({ active: () => active });
    expect(await service.handleFrame(frame({ bridge_epoch: "old" }))).toBe(false);
    expect(await service.handleFrame(frame({ operation_id: "missing" }))).toBe(false);
    await cache.update("attempt", row => ({ ...row, journalIncarnation: "old" }));
    expect(await service.handleFrame(frame())).toBe(false);
    await cache.update("attempt", row => ({ ...row, journalIncarnation: "journal" })); active = false;
    expect(await service.handleFrame(frame())).toBe(false);
    expect(await cache.get("attempt")).not.toHaveProperty("progress");
  });

  it("gates creation admission and progress on environments without affecting ordinary writes", async () => {
    await cache.put(intent()); await bind({ currentGreeting: { ...greeting, capabilities: ["bridge.epoch"] } });
    expect(await service.handleFrame(frame())).toBe(false);
    await expect(service.submit({ method: "workspace.create", params: { project_id: "P" } })).rejects.toMatchObject({ error_code: "write_unsupported" });
    expect(await cache.get("attempt")).not.toHaveProperty("progress");
  });

  it("ignores unrelated methods and malformed progress", async () => {
    await cache.put(intent({ request: { method: "tasks.update", params: {} } })); await bind();
    expect(await service.handleFrame(frame())).toBe(false);
    await cache.update("attempt", row => ({ ...row, request: intent().request }));
    for (const invalid of [null, {}, [{ step: "workspace", state: "succeeded" }], [{ state: "running" }]]) {
      expect(await service.handleFrame(frame({ progress: invalid }))).toBe(false);
    }
    expect(await cache.get("attempt")).not.toHaveProperty("progress");
  });
});
