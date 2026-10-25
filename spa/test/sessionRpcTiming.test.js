import { afterEach, expect, it, vi } from "vitest";
import { transportEpoch } from "./transportEpoch.js";

const timing = vi.hoisted(() => ({
  itemQueued: vi.fn(() => "token"), itemStart: vi.fn(), itemReceipt: vi.fn(), itemSettled: vi.fn(),
  unregister: vi.fn(), registerSqlCredits: vi.fn(),
}));
vi.mock("../src/core/syncDiagnostics.js", () => ({ syncDiagnostics: timing }));
import { createSessionRpc, ANSWER_TIMEOUT_MS, SQL_ADMISSION_TIMEOUT_MS } from "../src/core/sessionRpc.js";

const flush = async () => { for (let n = 0; n < 20; n += 1) await Promise.resolve(); };
const sessions = [];
function setup() {
  let receive;
  const wire = { sent: [], send(frame) { this.sent.push(frame.payload); }, onEnvelope(fn) { receive = fn; }, onClose() {} };
  const rpc = createSessionRpc({
    bridgeEpoch: transportEpoch(), sessionId: "private-session", deviceId: "private-device", sessionKeyB64: "key", sqlReadLimit: 1,
    transport: { encryptFrame: async ({ frameFields }) => frameFields, decryptEnvelope: async ({ envelope }) => envelope },
  });
  rpc.rideOn(wire);
  sessions.push(rpc);
  const ask = (method, options) => { const p = rpc.call(method, { body: "private-body" }, options); p.catch(() => {}); return p; };
  return { rpc, wire, ask, reply: (payload) => receive({ payload }) };
}
afterEach(() => {
  sessions.splice(0).forEach((rpc) => rpc.close());
  vi.useRealTimers();
  vi.clearAllMocks();
});

it("records queue, admission, first receipt and final reply synchronously without request data", async () => {
  timing.registerSqlCredits.mockReturnValue(timing.unregister);
  const { rpc, wire, ask, reply } = setup();
  ask("tasks.get", { timeoutMs: 0 });
  ask("tasks.list", { timeoutMs: 0 });
  expect(timing.itemQueued.mock.calls).toEqual([["tasks.get", "rpc"], ["tasks.list", "rpc"]]);
  expect(timing.itemStart).toHaveBeenCalledTimes(1);
  await flush();
  reply({ id: wire.sent[0].id, accepted: true });
  await flush();
  reply({ id: wire.sent[0].id, accepted: true });
  await flush();
  expect(timing.itemReceipt).toHaveBeenCalledExactlyOnceWith("token");
  reply({ id: wire.sent[0].id, ok: false, error: "private-error" });
  await flush();
  expect(timing.itemSettled).toHaveBeenCalledWith("token", "reply");
  expect(timing.itemStart).toHaveBeenCalledTimes(2);
  const snapshot = timing.registerSqlCredits.mock.calls[0][0];
  expect(snapshot()).toEqual({ queued: 0, admitted: 1, timedOutHeld: 0 });
  rpc.close();
  expect(timing.unregister).toHaveBeenCalled();
});

it.each([[false, "path-timeout"], [true, "answer-timeout"]])("records deadline outcome receipt=%s while retaining SQL credit", async (receipted, outcome) => {
  vi.useFakeTimers();
  const { rpc, wire, ask, reply } = setup();
  const pending = ask("tasks.get", { timeoutMs: 10 });
  await flush();
  if (receipted) { reply({ id: wire.sent[0].id, accepted: true }); await flush(); }
  await vi.advanceTimersByTimeAsync(receipted ? ANSWER_TIMEOUT_MS : 10);
  await expect(pending).rejects.toMatchObject({ timedOut: true });
  expect(timing.itemSettled).toHaveBeenCalledExactlyOnceWith("token", outcome);
  expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 0, admitted: 1, timedOutHeld: 1 });
});

it("does not classify an explicit caller abort as an admission deadline", async () => {
  const { ask } = setup();
  ask("tasks.get", { timeoutMs: 0 });
  const abort = new AbortController();
  const queued = ask("tasks.list", { signal: abort.signal, timeoutMs: 0 });
  abort.abort();
  await expect(queued).rejects.toMatchObject({ name: "AbortError" });
  expect(timing.itemSettled).toHaveBeenCalledWith("token", null);
  expect(timing.itemSettled.mock.calls.some(([, outcome]) => outcome === "admission-cancel")).toBe(false);
});


it.each([null, 0])("records admission-cancel when background SQL admission expires (timeoutMs=%s)", async (timeoutMs) => {
  vi.useFakeTimers();
  const { rpc, wire, ask, reply } = setup();
  const blocker = ask("tasks.get", { timeoutMs: 0 });
  await flush();
  const queued = ask("tasks.list", { priority: "background", timeoutMs });
  await flush();
  expect(timing.itemStart).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(SQL_ADMISSION_TIMEOUT_MS);
  await expect(queued).rejects.toMatchObject({ deadline: "admission", timedOut: true, uncertain: false, retryable: true });
  expect(timing.itemSettled).toHaveBeenCalledExactlyOnceWith("token", "admission-cancel");
  expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 0, admitted: 1, timedOutHeld: 0 });
  reply({ id: wire.sent[0].id, ok: true, result: {} });
  await blocker;
  await flush();
  expect(wire.sent).toHaveLength(1);
  expect(timing.itemStart).toHaveBeenCalledTimes(1);
});
