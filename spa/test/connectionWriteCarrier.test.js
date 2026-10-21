// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { openSession } from "../src/core/session.js";
import { createWriteOperationCache, wipePendingOperations } from "../src/core/writeOperationCache.js";
import { createWriteOperationService } from "../src/core/writeOperations.js";

vi.mock("../src/api.js", () => ({ fetchGatewayToken: async () => "token", fetchIceServers: async () => [], fetchDevices: async () => [] }));
vi.mock("../src/core/peerLink.js", () => ({ openPeerLink: async () => { throw new Error("no peer negotiation in this suite"); } }));
vi.mock("../src/terminal/manager.js", () => ({
  followTerminalDevice() {}, resetTerminalManager() {}, terminalDeviceId: () => null,
  terminalManager: () => null, subscribeTerminalStatus: () => () => {}, provideTerminalSessions() {},
}));
vi.mock("../src/core/composeView.js", () => ({ flushCaptures: async () => {} }));

const { resetApplication } = await import("../src/app.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { greetLiveBridge } = await import("../src/connection.js");
const { resetChangeEvents } = await import("../src/core/changeEvents.js");

const scope = { accountId: "carrier-account", deviceId: "carrier-device", principalId: "carrier-user" };
const operationId = "same-durable-operation";
const greeting = {
  api_version: "3.24.0", capabilities: ["bridge.epoch", "writes.status", "writes.subscribe"],
  bridge_epoch: "same-process", journal_incarnation: "same-journal", ack_confirm_methods: ["tasks.update"],
};
const transport = { encryptFrame: async ({ frameFields }) => ({ frameFields }), decryptEnvelope: async ({ envelope }) => envelope.frameFields };
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const resources = [];
const summary = (cursor, operations = []) => ({ journal_incarnation: greeting.journal_incarnation, cursor, operations });
const confirmation = (state, revision = 2) => ({
  type: "writes.confirmed", bridge_epoch: greeting.bridge_epoch, operation_id: operationId, revision, state,
  ...(state === "failed" ? { effect: "unapplied", error: { code: "stale", message: "Retired carrier" } } : { result: { saved: true } }),
});

function carrier() {
  const readers = new Set();
  const wire = {
    sent: [], onClose: () => () => {}, close() {},
    onEnvelope: (fn) => { readers.add(fn); return () => readers.delete(fn); },
    send({ frameFields }) {
      const request = frameFields.payload;
      wire.sent.push(request);
      if (request.method === "writes.subscribe") return;
      const result = request.method === "session.hello" ? greeting : request.method === "writes.status" ? summary("0") : {};
      queueMicrotask(() => wire.answer(request, result));
    },
    answer: (request, result) => wire.deliver({ id: request.id, ok: true, bridge_epoch: greeting.bridge_epoch, result }),
    deliver: (payload) => readers.forEach((fn) => fn({ frameFields: { payload } })),
  };
  return wire;
}

async function subscriptionOn(wire) {
  await vi.waitFor(() => expect(wire.sent.some((request) => request.method === "writes.subscribe")).toBe(true));
  return wire.sent.findLast((request) => request.method === "writes.subscribe");
}

async function replacedCarrier() {
  const old = carrier();
  const next = carrier();
  const session = await openSession({ deviceId: scope.deviceId, transport, rendezvous: {
    mint: async () => ({ sessionId: "same-opening", sessionKeyB64: "same-key" }), signalCarrier: carrier,
  } });
  const cache = createWriteOperationCache(scope);
  await cache.put({ operationId, request: { method: "tasks.update", params: { task_id: "task", body: "accepted edit" } },
    method: "tasks.update", acknowledged: true, state: "accepted", revision: 1, acceptedEpoch: greeting.bridge_epoch,
    journalIncarnation: greeting.journal_incarnation, overlayActive: true, overlay: [], draft: { body: "accepted edit" } });
  const service = createWriteOperationService({ ...scope, cache, setTimeout: () => 1, clearTimeout() {} });
  resources.push({ session, service });
  const bindings = [];
  const bind = service.bind;
  vi.spyOn(service, "bind").mockImplementation((input) => {
    const binding = bind(input);
    bindings.push(binding);
    return binding;
  });
  const context = adoptDeviceSession(session);
  context.writeOperations = service;
  session.onCarrier(() => greetLiveBridge(context, { suppressFailure: false }));
  await session.peer(old);
  const oldSubscription = await subscriptionOn(old);
  const authority = session.bridgeEpoch.current();
  await session.peer(next);
  const newSubscription = await subscriptionOn(next);
  expect(session.bridgeEpoch.current()).toMatchObject({ bridge_epoch: authority.bridge_epoch, opening: authority.opening });
  expect(service.bind).toHaveBeenCalledTimes(2);
  for (const wire of [old, next]) {
    expect(wire.sent.filter((request) => request.method.startsWith("writes.")).map((request) => [request.method, request.params.operation_ids]))
      .toEqual([["writes.status", [operationId]], ["writes.subscribe", [operationId]]]);
    expect(wire.sent.some((request) => request.method === "tasks.update")).toBe(false);
  }
  return { session, cache, service, old, next, oldSubscription, newSubscription, bindings };
}

beforeEach(async () => {
  resetApplication(); resetChangeEvents(); resetDeviceContexts();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  await wipePendingOperations();
});
afterEach(async () => {
  for (const { session, service } of resources.splice(0)) { session.close(); await service.close(); }
  resetChangeEvents(); resetDeviceContexts();
});

describe("same-process write subscription carrier replacement", () => {
  it("reconciles on the new carrier and rejects an old carrier answering its new subscribe request", async () => {
    const { cache, old, next, oldSubscription, newSubscription, bindings } = await replacedCarrier();
    old.answer(oldSubscription, summary("8", [confirmation("failed", 8)]));
    old.answer(newSubscription, summary("9", [confirmation("failed", 9)]));
    next.answer(newSubscription, summary("1"));
    await Promise.all(bindings);
    expect(await cache.get(operationId)).toMatchObject({ operationId, revision: 1, state: "accepted", draft: { body: "accepted edit" } });
  });

  it("fences old subscription pushes before they can settle the new binding", async () => {
    const { service, cache, old, next, oldSubscription, newSubscription, bindings } = await replacedCarrier();
    old.answer(oldSubscription, summary("0"));
    next.answer(newSubscription, summary("1"));
    await Promise.all(bindings);
    old.deliver(confirmation("failed"));
    next.deliver(confirmation("succeeded"));
    await tick();
    await service.handleFrame(null);
    expect(await cache.get(operationId)).toMatchObject({ operationId, revision: 2, state: "succeeded" });
  });
});
