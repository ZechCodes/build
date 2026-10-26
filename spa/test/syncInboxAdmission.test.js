import { afterEach, expect, it, vi } from "vitest";
import { createSyncPhases } from "../src/core/syncPhases.js";
import { createBackgroundStreams } from "../src/core/syncBackgroundStreams.js";
import { createSessionRpc, SQL_ADMISSION_TIMEOUT_MS } from "../src/core/sessionRpc.js";
import { SQL_READ_LIMIT } from "../src/core/sqlReadCredits.js";
import { transportEpoch } from "./transportEpoch.js";

vi.mock("../src/core/localCache.js", () => ({ cachedSubKeys: async () => [] }));
const flush = async () => { for (let turn = 0; turn < 100; turn += 1) await Promise.resolve(); };
const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};
const sessions = [];
afterEach(() => {
  sessions.splice(0).forEach((rpc) => rpc.close());
  vi.useRealTimers();
});

function connection() {
  let receive;
  const wire = {
    sent: [], send(frame) { this.sent.push(frame.payload); },
    onEnvelope(fn) { receive = fn; }, onClose() {},
  };
  const rpc = createSessionRpc({
    bridgeEpoch: transportEpoch(), sessionId: "sync-pass", deviceId: "dev-1", sessionKeyB64: "key",
    transport: { encryptFrame: async ({ frameFields }) => frameFields, decryptEnvelope: async ({ envelope }) => envelope },
  });
  rpc.rideOn(wire);
  sessions.push(rpc);
  const ask = (method, params = {}, options = {}) => {
    const pending = rpc.call(method, params, { timeoutMs: 0, ...options });
    pending.catch(() => {});
    return pending;
  };
  return { rpc, wire, ask, reply: (request) => receive({ payload: { id: request.id, ok: true, result: {} } }) };
}

function planner(streams, ask, siblingReady, failures) {
  const row = { run_id: "run-1", agents: [{ id: "first" }, { id: "sibling" }] };
  const context = { deviceId: "dev-1", active: () => true, backgroundStreams: streams };
  let phases;
  const thread = vi.fn(async (_context, entityId, agent) => {
    if (agent.id === "sibling") await siblingReady.promise;
    const scope = { deviceId: "dev-1", entityId, kind: "thread", sub: agent.id };
    try {
      await streams.read({
        active: context.active,
        priority: () => phases.priorityForRead(scope, { priority: "background" }),
        before: () => phases.beforeBackgroundRead(scope),
        load: (envelope) => ask("thread.page", { agent_id: agent.id }, envelope),
      });
    } catch (error) { failures.push({ agent: agent.id, error }); }
  });
  const nothing = async () => {};
  phases = createSyncPhases({
    context, view: { items: [row], projects: [] }, route: () => ({ name: "inbox", deviceId: "dev-1" }),
    pass: { order: ["run-1"], rows: new Map([["run-1", row]]) }, entityContext: () => context,
    readers: { scope: () => null, thread, terminals: nothing, extras: async () => true },
    tasks: { first: nothing, rest: nothing },
  });
  return { phases, thread };
}

it("returns a timed-out Inbox worker's slot while its sibling shares the eight SQL credits and carries on", async () => {
  vi.useFakeTimers();
  const { rpc, wire, ask, reply } = connection();
  const streams = createBackgroundStreams().forContext({ deviceId: "dev-1", session: rpc });
  const blockers = Array.from({ length: SQL_READ_LIMIT }, () => ask("tasks.get"));
  await flush();
  expect(wire.sent).toHaveLength(8);
  const siblingReady = gate(), failures = [];
  const { phases, thread } = planner(streams, ask, siblingReady, failures);
  const pass = phases.run();
  pass.catch(() => {});
  try {
    await flush();
    expect(thread.mock.calls.map(([, , agent]) => agent.id)).toEqual(["first", "sibling"]);
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 1, admitted: 8, timedOutHeld: 0 });
    // Stagger only test admission, so the first deadline cannot expire its sibling.
    await vi.advanceTimersByTimeAsync(SQL_ADMISSION_TIMEOUT_MS / 2);
    siblingReady.release();
    await flush();
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 2, admitted: 8, timedOutHeld: 0 });
    const probe = streams.read({
      active: () => true, priority: () => ({ priority: "background" }),
      load: (envelope) => ask("ping", {}, envelope),
    });
    await flush();
    expect(wire.sent.some((request) => request.method === "ping")).toBe(false);
    const write = ask("tasks.comment", { body: "user write" });
    await flush();
    const writeRequest = wire.sent.find((request) => request.method === "tasks.comment");
    expect(writeRequest).toBeDefined();
    reply(writeRequest);
    await write;
    await vi.advanceTimersByTimeAsync(SQL_ADMISSION_TIMEOUT_MS / 2);
    await flush();
    expect(failures).toHaveLength(1);
    expect(failures[0]).toMatchObject({ agent: "first", error: { deadline: "admission", uncertain: false, retryable: true } });
    const probeRequest = wire.sent.find((request) => request.method === "ping");
    expect(probeRequest).toBeDefined();
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 1, admitted: 8, timedOutHeld: 0 });
    reply(probeRequest);
    await probe;
    reply(wire.sent[0]);
    await flush();
    const siblingRequest = wire.sent.find((request) => request.method === "thread.page");
    expect(siblingRequest.params.agent_id).toBe("sibling");
    expect(wire.sent.some((request) => request.params?.agent_id === "first")).toBe(false);
    reply(siblingRequest);
    await expect(pass).resolves.toBe(true);
    expect(failures).toHaveLength(1);
  } finally {
    siblingReady.release();
    rpc.close();
    await Promise.allSettled([...blockers, pass]);
  }
});
