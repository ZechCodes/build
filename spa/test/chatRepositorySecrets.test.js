// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

function setupIdb() {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
}

const identity = {
  entityId: "owner-chat", agentId: "agent-chat", conversationId: "conversation-chat", threadId: "thread-chat", threadGenerationRevision: 1,
};
const descriptor = {
  secret_id: "sec_chat", request_id: "11111111-1111-4111-8111-111111111111", name: "CHAT_TOKEN", ttl_seconds: null,
  grant: { scope: "agent", agent_id: "agent-chat", conversation_id: "conversation-chat", generation: "thread-chat" },
  scope: { owner_id: "owner-chat", conversation_id: "conversation-chat", generation: "thread-chat", draft_id: null },
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

it("restores and captures staged ids before a fresh repository greets", async () => {
  vi.resetModules();
  setupIdb();
  const [{ createChatRepository }, { readSecretSupport }] = await Promise.all([
    import("../src/core/chatRepository.js"),
    import("../src/core/secretSupport.js"),
  ]);
  const capable = {
    api_version: "3.36.0",
    capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
  };
  const call = vi.fn(async () => ({ phase: "ack", operation_id: "op-cold" }));
  const first = createChatRepository({ scope: { deviceId: "device-cold" }, call, createOperationId: () => "op-cold" });
  first.configureCapabilities(capable);
  const initial = first.controller(identity);
  initial.writeDraft({ body: "use it", secrets: [descriptor] });
  await vi.waitFor(async () => expect((await readSecretSupport("device-cold")).attachments).toBe(true));
  initial.dispose();

  const reloaded = createChatRepository({ scope: { deviceId: "device-cold" }, call, createOperationId: () => "op-cold" });
  const restored = reloaded.controller(identity);
  await vi.waitFor(() => expect(restored.readDraft().secrets).toEqual([descriptor]));
  const submission = restored.captureSubmission();
  expect(submission.originalDraft.secrets).toEqual([descriptor]);
  expect(submission.message.secret_ids).toEqual(["sec_chat"]);
  reloaded.configureCapabilities(capable);
  await restored.post(submission);
  expect(call).toHaveBeenCalledWith("thread.post", expect.objectContaining({ secret_ids: ["sec_chat"] }), expect.anything());
});

it("refuses cached secret support before any current greeting authorizes dispatch", async () => {
  vi.resetModules();
  setupIdb();
  const [{ createChatRepository }, { rememberSecretSupport }] = await Promise.all([
    import("../src/core/chatRepository.js"),
    import("../src/core/secretSupport.js"),
  ]);
  await rememberSecretSupport("device-ungreeted", { secrets: {
    attachments: true, sealedUpload: true, metadataEvents: true,
  } });
  const call = vi.fn(async () => ({ phase: "ack", operation_id: "op-ungreeted" }));
  const repository = createChatRepository({
    scope: { deviceId: "device-ungreeted" }, call, createOperationId: () => "op-ungreeted", storage: null,
  });
  const controller = repository.controller(identity);
  await vi.waitFor(() => expect(repository.secretAttachments()).toBe(true));
  controller.writeDraft({ body: "keep it", secrets: [descriptor] });
  const submission = controller.captureSubmission();

  await expect(controller.post(submission)).rejects.toMatchObject({ code: "unknown_method" });

  expect(call).not.toHaveBeenCalled();
  expect(controller.restoreRejected(submission, new Error("ungreeted"))).toBe("restored");
  expect(controller.readDraft()).toMatchObject({ body: "keep it", secrets: [descriptor] });
});

it("refuses cached secret ids intact when the current greeting does not support them", async () => {
  vi.resetModules();
  setupIdb();
  const [{ createChatRepository }, { rememberSecretSupport }] = await Promise.all([
    import("../src/core/chatRepository.js"),
    import("../src/core/secretSupport.js"),
  ]);
  await rememberSecretSupport("device-downgrade", { secrets: { attachments: true, sealedUpload: true, metadataEvents: true } });
  const call = vi.fn(async () => ({ phase: "ack", operation_id: "op-old" }));
  const repository = createChatRepository({ scope: { deviceId: "device-downgrade" }, call, createOperationId: () => "op-old", storage: null });
  const controller = repository.controller(identity);
  await vi.waitFor(() => expect(repository.secretAttachments()).toBe(true));
  controller.writeDraft({ body: "ordinary", secrets: [descriptor] });
  const submission = controller.captureSubmission();
  repository.configureCapabilities({ api_version: "3.34.0", capabilities: [] });

  await expect(controller.post(submission)).rejects.toMatchObject({ code: "unknown_method" });

  expect(call).not.toHaveBeenCalled();
  expect(controller.restoreRejected(submission, new Error("unsupported"))).toBe("restored");
  expect(controller.readDraft()).toMatchObject({ body: "ordinary", secrets: [descriptor] });
  await expect(controller.post(controller.captureSubmission())).rejects.toMatchObject({ code: "unknown_method" });
  expect(call).not.toHaveBeenCalled();
});

it("does not carry a secret draft into a replacement thread generation", async () => {
  vi.resetModules();
  setupIdb();
  const { createChatRepository } = await import("../src/core/chatRepository.js");
  const repository = createChatRepository({ scope: { deviceId: "device-generation" }, call: vi.fn() });
  repository.configureCapabilities({
    api_version: "3.36.0",
    capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
  });
  const old = repository.controller(identity);
  old.writeDraft({ body: "old", secrets: [descriptor] });
  repository.observeThreadGeneration(identity.conversationId, "thread-new", 2);
  const current = repository.controller({ ...identity, threadId: "thread-new", threadGenerationRevision: 2 });

  expect(current.readDraft().secrets).toEqual([]);
  expect(() => old.captureSubmission()).toThrow("Conversation was cleared");
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
  const broadcasts = [];
  vi.stubGlobal("BroadcastChannel", class {
    postMessage(message) { broadcasts.push(structuredClone(message)); }
    addEventListener() {}
    close() {}
  });
  const [
    { createChatRepository }, { createSecretDraftService },
    { createSecretMetadataCache, secretMetadataAddress }, { readUiRecord }, { readCached }, diagnostics,
  ] = await Promise.all([
    import("../src/core/chatRepository.js"),
    import("../src/core/secretDrafts.js"),
    import("../src/core/secretMetadataCache.js"),
    import("../src/core/localUiStore.js"),
    import("../src/core/localCache.js"),
    import("../src/core/connectionDiagnostics.js"),
  ]);
  diagnostics.clearConnectionDiagnosticHistory();
  const consoleSpies = ["debug", "info", "log", "warn", "error"]
    .map((method) => vi.spyOn(console, method).mockImplementation(() => {}));
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
  const metadata = createSecretMetadataCache({
    deviceId: "device-sinks", call, capabilities: () => ({ attachments: true, sealedUpload: true, metadataEvents: true }),
  });
  const service = createSecretDraftService({
    deviceId: "device-sinks", creator: "device_user", projectId: "project-sinks",
    recipient: () => ({ ownerId: "owner-chat", agentId: "agent-chat", conversationId: "conversation-chat", generation: "thread-chat", draftId: null }),
    capabilities: () => ({ attachments: true, sealedUpload: true, metadataEvents: true }),
    deviceTransportPublicKey: () => "synthetic-public-key", call, draft: controller.secretDraft(), metadata,
    sealUpload: vi.fn(async () => "synthetic-sealed-envelope"),
    createRequestId: () => descriptor.request_id,
  });
  const sentinel = "synthetic-plaintext-only-in-modal";
  await service.stage({ name: "CHAT_TOKEN", value: sentinel, scope: "agent" });
  await vi.waitFor(async () => expect((await readUiRecord(repository.draftAddress(identity)))?.value?.secrets).toEqual([descriptor]));

  const stored = await readUiRecord(repository.draftAddress(identity));
  const metadataStored = await readCached(secretMetadataAddress("device-sinks", {
    projectId: "project-sinks", ownerId: "owner-chat", agentId: "agent-chat",
    conversationId: "conversation-chat", generation: "thread-chat", draftId: null,
  }));
  const localValues = Array.from({ length: localStorage.length }, (_, index) => localStorage.getItem(localStorage.key(index)));
  expect(JSON.stringify({ stored, metadataStored, localValues })).not.toContain(sentinel);
  expect(JSON.stringify(diagnostics.connectionDiagnosticHistory())).not.toContain(sentinel);
  expect(JSON.stringify(consoleSpies.flatMap((spy) => spy.mock.calls))).not.toContain(sentinel);
  expect(JSON.stringify(broadcasts)).not.toContain(sentinel);
  expect(JSON.stringify(broadcasts)).not.toContain("synthetic-sealed-envelope");
  const submission = controller.captureSubmission({ ...controller.readDraft(), body: "use the secret" });
  await controller.post(submission);
  const post = call.mock.calls.find(([method]) => method === "thread.post");
  expect(JSON.stringify(post)).not.toContain(sentinel);
  expect(JSON.stringify(post)).not.toContain("synthetic-sealed-envelope");
  metadata.dispose();
  consoleSpies.forEach((spy) => spy.mockRestore());
  vi.unstubAllGlobals();
});

it("submits a secret-bearing post immediately through ack-then-confirm", async () => {
  vi.resetModules();
  const [{ createChatRepository }, v1] = await Promise.all([
    import("../src/core/chatRepository.js"),
    import("../src/core/bridgeApi/v1/index.js"),
  ]);
  const greeting = {
    api_version: "3.36.0",
    capabilities: ["bridge.epoch", "secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
    ack_confirm_methods: ["thread.post"],
  };
  const rawCall = vi.fn();
  const writes = { submit: vi.fn(async () => ({ accepted: true })) };
  const adapter = v1.create(rawCall, greeting, { writeOperations: writes });
  const repository = createChatRepository({
    scope: { deviceId: "device-ack" }, call: adapter.call,
    createOperationId: () => "operation-ack",
    storage: { getItem: () => null, setItem: vi.fn() },
  });
  repository.configureCapabilities(greeting);
  const controller = repository.controller(identity);
  controller.writeDraft({ body: "atomic", secrets: [descriptor] });

  const receipt = await controller.post(controller.captureSubmission());

  expect(receipt).toMatchObject({ phase: "ack", operation_id: "operation-ack" });
  expect(writes.submit).toHaveBeenCalledOnce();
  expect(writes.submit).toHaveBeenCalledWith(expect.objectContaining({
    method: "thread.post",
    operationId: "operation-ack",
    params: expect.objectContaining({ body: "atomic", secret_ids: ["sec_chat"], operation_id: "operation-ack" }),
  }));
  expect(rawCall).not.toHaveBeenCalled();
});

it("keeps plaintext out of the durable write-operation IndexedDB record", async () => {
  vi.resetModules();
  setupIdb();
  const [
    { createChatRepository }, { createSecretDraftService }, { createWriteOperationCache },
    { createWriteOperationService }, v1,
  ] = await Promise.all([
    import("../src/core/chatRepository.js"),
    import("../src/core/secretDrafts.js"),
    import("../src/core/writeOperationCache.js"),
    import("../src/core/writeOperations.js"),
    import("../src/core/bridgeApi/v1/index.js"),
  ]);
  const operationScope = {
    accountId: "11111111-1111-4111-8111-111111111111",
    deviceId: "device-durable-secret",
    principalId: "user:11111111-1111-4111-8111-111111111111",
  };
  const cache = createWriteOperationCache(operationScope);
  const writes = createWriteOperationService({ ...operationScope, cache, randomUUID: () => "operation-secret" });
  const greeting = {
    api_version: "3.36.0", bridge_epoch: "epoch-secret", journal_incarnation: "journal-secret",
    capabilities: ["bridge.epoch", "secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
    ack_confirm_methods: ["thread.post"],
  };
  const session = {
    callRaw: vi.fn(async (method) => method.startsWith("writes.")
      ? { journal_incarnation: "journal-secret", operations: [], cursor: "cursor-secret" }
      : {
        ok: true, bridge_epoch: "epoch-secret",
        write: { phase: "ack", operation_id: "operation-secret", revision: 1, state: "accepted", accepted_epoch: "epoch-secret" },
        result: { phase: "ack", operation_id: "operation-secret" },
      }),
    onWriteFrame: () => () => {},
  };
  await writes.bind({ session, greeting, active: () => true });
  const adapter = v1.create(session.callRaw, greeting, { writeOperations: writes });
  const repository = createChatRepository({
    scope: { deviceId: operationScope.deviceId }, call: adapter.call, writeOperations: writes,
    storage: null, createOperationId: () => "operation-secret",
  });
  repository.configureCapabilities(greeting);
  const controller = repository.controller(identity);
  const stageCall = vi.fn(async () => descriptor);
  const service = createSecretDraftService({
    deviceId: operationScope.deviceId, creator: "device_user", projectId: "project-sinks",
    recipient: () => ({ ownerId: "owner-chat", agentId: "agent-chat", conversationId: "conversation-chat", generation: "thread-chat", draftId: null }),
    capabilities: () => ({ attachments: true, sealedUpload: true, metadataEvents: true }),
    deviceTransportPublicKey: () => "synthetic-public-key", call: stageCall, draft: controller.secretDraft(),
    metadata: { upsert: vi.fn(async () => descriptor) }, sealUpload: vi.fn(async () => "synthetic-sealed-envelope"),
    createRequestId: () => descriptor.request_id,
  });
  const sentinel = "synthetic-durable-plaintext";
  await service.stage({ name: "CHAT_TOKEN", value: sentinel, scope: "agent" });
  await controller.post(controller.captureSubmission({ ...controller.readDraft(), body: "use it" }));

  const stored = await writes.get("operation-secret");
  expect(stored).toMatchObject({
    request: { method: "thread.post", params: expect.objectContaining({ secret_ids: ["sec_chat"] }) },
    draft: expect.objectContaining({ secrets: [descriptor] }),
  });
  expect(JSON.stringify(stored)).not.toContain(sentinel);
  expect(JSON.stringify(stored)).not.toContain("synthetic-sealed-envelope");
  await writes.close();
});
