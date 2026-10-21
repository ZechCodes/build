import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { openSession } from "../src/core/session.js";
import { transportEpoch } from "./transportEpoch.js";
import { selectAdapter } from "../src/core/bridgeApi/index.js";
import { createChatRepository } from "../src/core/chatRepository.js";
import { createWriteOperationCache, wipePendingOperations } from "../src/core/writeOperationCache.js";
import { createWriteOperationService } from "../src/core/writeOperations.js";
import { normalizeAckEffects, normalizeIntentEffects } from "../src/core/writeOperationEffects.js";
import { threadCacheAddress, writeProvisionalMessage } from "../src/core/conversationCache.js";
import { readCached, readCanonicalCached, writeCached, wipeCache } from "../src/core/localCache.js";

const scope = { accountId: "account-prehello", deviceId: "device-prehello", principalId: "user-prehello" };
const identity = { entityId: "entity", agentId: "agent", conversationId: "conversation" };
const address = threadCacheAddress({ deviceId: scope.deviceId, ...identity });
const greeting = { api_version: "3.24.0", capabilities: ["bridge.epoch"], bridge_epoch: "epoch", journal_incarnation: "journal", ack_confirm_methods: ["thread.post"] };
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const resources = [];
function carrier() {
  const listeners = new Set();
  return { sent: [], onEnvelope: (fn) => { listeners.add(fn); return () => listeners.delete(fn); }, onClose: () => () => {}, close() {},
    send(envelope) { this.sent.push(envelope); }, reply(payload) { listeners.forEach((fn) => fn({ frameFields: { payload } })); } };
}
const transport = { encryptFrame: async ({ frameFields }) => ({ frameFields }), decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }) };
async function queuedChat() {
  const signal = carrier(), peer = carrier();
  const rendezvous = { mint: async () => ({ sessionId: "session", sessionKeyB64: "key" }), signalCarrier: () => signal, onClosed: () => () => {} };
  const session = await openSession({ rendezvous, deviceId: scope.deviceId, bridgeEpoch: transportEpoch(), transport });
  await session.peer(peer);
  const cache = createWriteOperationCache(scope);
  const service = createWriteOperationService({ ...scope, cache, normalizeAckEffects, normalizeIntentEffects, setTimeout: () => 1, clearTimeout: () => {} });
  const repository = createChatRepository({ scope, call: session.call, writeOperations: service, storage: null, createOperationId: () => "message-before-hello" });
  resources.push({ session, service, repository });
  await writeCached(address, { items: [], deliveredSequence: 0 });
  const controller = repository.controller(identity);
  controller.writeDraft({ body: "keep my prehello message" });
  const submission = controller.captureSubmission();
  if (!submission.durable) await writeProvisionalMessage(address, submission.operationId, submission.message);
  const posted = controller.post(submission);
  void posted.catch(() => {});
  await tick();
  expect(peer.sent).toHaveLength(0);
  return { session, peer, service, cache, controller, submission, posted };
}
beforeEach(async () => { globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange; await wipeCache(); await wipePendingOperations(); });
afterEach(async () => { for (const { session, service, repository } of resources.splice(0)) { repository.dispose(); session.close(); await service.close(); } });

describe("a chat send queued before its first greeting", () => {
  it("keeps the existing nonallowlisted final-reply path when the greeting enables no durable methods", async () => {
    const { session, peer, service, cache, submission, posted } = await queuedChat();
    const legacyGreeting = { ...greeting, ack_confirm_methods: [] };
    await service.bind({ session, greeting: legacyGreeting });
    session.installAdapter(selectAdapter(legacyGreeting), { writeOperations: service });
    let request;
    for (let count = 0; count < 100 && !request; count += 1) { await tick(); request = peer.sent.map((entry) => entry.frameFields.payload).find((frame) => frame.method === "thread.post"); }
    expect(request).toBeDefined();
    expect(request).not.toHaveProperty("write");
    expect(request.params).not.toHaveProperty("draft");
    expect(await cache.get(submission.operationId)).toBeUndefined();
    expect((await readCanonicalCached(address)).value.items).toHaveLength(1);
    peer.reply({ id: request.id, ok: true, bridge_epoch: "epoch", result: { operation_id: submission.operationId, posted_sequence: 1 } });
    await expect(posted).resolves.toMatchObject({ posted_sequence: 1 });
  });

  it("persists its draft through the real adapter gate and migrates its legacy stand-in before durable transport", async () => {
    const { session, peer, service, cache, controller, submission, posted } = await queuedChat();
    await service.bind({ session, greeting });
    session.installAdapter(selectAdapter(greeting), { writeOperations: service });
    let request;
    for (let count = 0; count < 100 && !request; count += 1) { await tick(); request = peer.sent.map((entry) => entry.frameFields.payload).find((frame) => frame.method === "thread.post"); }
    expect(request).toBeDefined();
    const saved = await cache.get(submission.operationId);
    expect(saved).toMatchObject({ operationId: submission.operationId, owner: { kind: "chat", ...identity }, draft: { body: "keep my prehello message" } });
    expect((await readCanonicalCached(address)).value.items).toEqual([]);
    expect((await readCached(address)).value.items).toHaveLength(1);
    peer.reply({ id: request.id, ok: true, bridge_epoch: "epoch", write: { phase: "ack", state: "accepted", operation_id: submission.operationId, accepted_epoch: "epoch", revision: 1 }, result: {} });
    await expect(posted).resolves.toMatchObject({ phase: "ack", operation_id: submission.operationId });
    await service.handleFrame({ type: "writes.confirmed", bridge_epoch: "epoch", operation_id: submission.operationId, revision: 2, state: "failed", effect: "unapplied", error: { message: "not saved" } });
    expect((await readCached(address)).value.items).toEqual([]);
    expect(controller.readDraft().body).toBe("keep my prehello message");
    expect(controller.recoveries()).toEqual([expect.objectContaining({ operationId: submission.operationId, status: "rejected" })]);
  });
});
