// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

function setupIdb() {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
}

const identity = {
  entityId: "owner-chat", agentId: "agent-chat", conversationId: "agent-chat", threadId: "thread-chat", threadGenerationRevision: 1,
};
const descriptor = {
  secret_id: "sec_chat", request_id: "11111111-1111-4111-8111-111111111111", name: "CHAT_TOKEN", ttl_seconds: null,
  grant: { scope: "agent", agent_id: "agent-chat", conversation_id: "agent-chat", generation: "thread-chat" },
  scope: { owner_id: "owner-chat", conversation_id: "agent-chat", generation: "thread-chat", draft_id: null },
  created_at: 1_800_000_000, attached_at: null, expires_at: null, revoked_at: null, revision: 1, status: "staged",
};

it("restores staged metadata after reload and sends ids atomically with the message", async () => {
  vi.resetModules();
  setupIdb();
  const { createChatRepository } = await import("../src/core/chatRepository.js");
  const call = vi.fn(async () => ({ phase: "ack", operation_id: "op-chat" }));
  const first = createChatRepository({ scope: { deviceId: "device-chat" }, call, createOperationId: () => "op-chat" });
  first.configureCapabilities({ api_version: "3.36.0", capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"] });
  const controller = first.controller(identity);
  controller.writeDraft({ body: "use it", secrets: [descriptor] });
  await vi.waitFor(() => expect(controller.readDraft().secrets).toEqual([descriptor]));
  controller.dispose();

  const reloaded = createChatRepository({ scope: { deviceId: "device-chat" }, call, createOperationId: () => "op-chat" });
  reloaded.configureCapabilities({ api_version: "3.36.0", capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"] });
  const restored = reloaded.controller(identity);
  await vi.waitFor(() => expect(restored.readDraft().secrets).toEqual([descriptor]));
  const submission = restored.captureSubmission();
  await restored.post(submission);
  expect(call).toHaveBeenCalledWith("thread.post", expect.objectContaining({
    body: "use it", secret_ids: ["sec_chat"], operation_id: "op-chat",
  }), expect.any(Object));
  const params = call.mock.calls[0][1];
  expect(params).not.toHaveProperty("secrets");
  expect(JSON.stringify(params)).not.toContain("value");
});

it("keeps a secret-bearing message out of old bridges without an error surface", async () => {
  vi.resetModules();
  setupIdb();
  const { createChatRepository } = await import("../src/core/chatRepository.js");
  const call = vi.fn();
  const repository = createChatRepository({ scope: { deviceId: "device-old" }, call });
  repository.configureCapabilities({ api_version: "3.34.0", capabilities: [] });
  const controller = repository.controller(identity);
  controller.writeDraft({ body: "ordinary", secrets: [descriptor] });
  expect(controller.secretDraft().read()).toEqual([]);
  const submission = controller.captureSubmission();
  expect(submission.message).toEqual(expect.objectContaining({ body: "ordinary" }));
  expect(submission.message).not.toHaveProperty("secret_ids");
});

it("restores the whole secret set after a refused atomic post", async () => {
  vi.resetModules();
  setupIdb();
  const { createChatRepository } = await import("../src/core/chatRepository.js");
  const repository = createChatRepository({ scope: { deviceId: "device-refused" }, call: vi.fn() });
  repository.configureCapabilities({
    api_version: "3.36.0",
    capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
  });
  const controller = repository.controller(identity);
  const second = { ...descriptor, secret_id: "sec_chat_second", request_id: "22222222-2222-4222-8222-222222222222", name: "SECOND_TOKEN" };
  controller.writeDraft({ body: "all or none", secrets: [descriptor, second] });
  const submission = controller.captureSubmission();
  expect(submission.message.secret_ids).toEqual(["sec_chat", "sec_chat_second"]);
  expect(controller.restoreRejected(submission, new Error("refused"))).toBe("restored");
  expect(controller.readDraft()).toMatchObject({ body: "all or none", secrets: [descriptor, second] });
});

it("keeps a staged value out of IndexedDB, local storage, and the post intent", async () => {
  vi.resetModules();
  setupIdb();
  localStorage.clear();
  const [{ createChatRepository }, { createSecretDraftService }, { readUiRecord }] = await Promise.all([
    import("../src/core/chatRepository.js"),
    import("../src/core/secretDrafts.js"),
    import("../src/core/localUiStore.js"),
  ]);
  const call = vi.fn(async (method) => method === "secrets.create"
    ? descriptor : { phase: "ack", operation_id: "operation-secret" });
  const repository = createChatRepository({
    scope: { deviceId: "device-sinks" }, call, createOperationId: () => "operation-secret",
  });
  repository.configureCapabilities({
    api_version: "3.36.0",
    capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
  });
  const controller = repository.controller(identity);
  const metadata = { upsert: vi.fn(async () => descriptor), refresh: vi.fn(), findRequest: vi.fn() };
  const service = createSecretDraftService({
    deviceId: "device-sinks", creator: "device_user", projectId: "project-sinks",
    recipient: () => ({ ownerId: "owner-chat", agentId: "agent-chat", conversationId: "agent-chat", generation: "thread-chat", draftId: null }),
    capabilities: () => ({ attachments: true, sealedUpload: true, metadataEvents: true }),
    deviceTransportPublicKey: () => "synthetic-public-key", call, draft: controller.secretDraft(), metadata,
    sealUpload: vi.fn(async () => "synthetic-sealed-envelope"),
    createRequestId: () => descriptor.request_id,
  });
  const sentinel = "synthetic-plaintext-only-in-modal";
  await service.stage({ name: "CHAT_TOKEN", value: sentinel, scope: "agent" });
  await vi.waitFor(async () => expect((await readUiRecord(repository.draftAddress(identity)))?.value?.secrets).toEqual([descriptor]));

  const stored = await readUiRecord(repository.draftAddress(identity));
  const localValues = Array.from({ length: localStorage.length }, (_, index) => localStorage.getItem(localStorage.key(index)));
  expect(JSON.stringify({ stored, localValues })).not.toContain(sentinel);
  const submission = controller.captureSubmission({ ...controller.readDraft(), body: "use the secret" });
  await controller.post(submission);
  const post = call.mock.calls.find(([method]) => method === "thread.post");
  expect(JSON.stringify(post)).not.toContain(sentinel);
  expect(JSON.stringify(post)).not.toContain("synthetic-sealed-envelope");
});
