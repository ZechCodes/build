import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createChatRepository } from "../src/core/chatRepository.js";

const address = (over = {}) => ({
  entityId: "run-1",
  agentId: "agent-1",
  conversationId: "conversation-1",
  ...over,
});

const createRepository = (call = vi.fn(async () => ({}))) => {
  let operation = 0;
  return createChatRepository({
    scope: { accountId: "account-1", deviceId: "device-1" },
    call,
    createOperationId: () => `operation-${++operation}`,
  });
};

const operationContract = JSON.parse(readFileSync(resolve("../fixtures/chat_operation_contract.json"), "utf8"));

describe("chat controller ownership", () => {
  it("returns one private controller per agent while sharing canonical history identity", () => {
    const repository = createRepository();
    const first = repository.controller(address());
    const same = repository.controller(address());
    const second = repository.controller(address({ agentId: "agent-2" }));

    first.writeDraft({ body: "first agent only" });

    expect(same).toBe(first);
    expect(second).not.toBe(first);
    expect(second.readDraft().body).toBe("");
    expect(first.historyIdentity).toBe(second.historyIdentity);
    expect(first.history.threadCache).toBe(second.history.threadCache);
    expect(first.threadState).toBe(second.threadState);
    expect(first.threadState.ownerId).toBe(first.historyIdentity);
    expect(first.identity.agentId).toBe("agent-1");
    expect(second.identity.agentId).toBe("agent-2");
  });

  it("gives every new-agent composition an independent provisional identity", () => {
    const repository = createRepository();
    const first = repository.createProvisional({ entityId: "run-1", conversationId: "conversation-1" });
    const second = repository.createProvisional({ entityId: "run-1", conversationId: "conversation-1" });

    first.writeDraft({ body: "draft one" });

    expect(first.identity.draftId).not.toBe(second.identity.draftId);
    expect(second.readDraft().body).toBe("");
  });

  it("binds a provisional controller once and promotes it to canonical history state", () => {
    const repository = createRepository();
    const controller = repository.createProvisional({ entityId: "run-1", conversationId: "draft-history" });
    const provisionalState = controller.threadState;

    repository.resolveProvisional(controller, address());

    expect(controller.identity).toMatchObject(address());
    expect(controller.threadState).toBe(repository.controller(address()).threadState);
    expect(controller.threadState).not.toBe(provisionalState);
    expect(() => repository.resolveProvisional(controller, address({ agentId: "agent-2" })))
      .toThrow("already bound");
  });

  it("captures destination, call target, settings revision, words, and attachments at submit", async () => {
    const oldCall = vi.fn(async (_method, params) => ({
      operation_id: params.operation_id,
      entity_id: params.entity_id,
      agent_id: params.agent_id,
      posted_sequence: 8,
      operation_status: "queued",
    }));
    const newCall = vi.fn();
    const repository = createRepository(oldCall);
    const controller = repository.controller(address());
    controller.absorbAgent({ model: "one", effort: "high", choice_revision: 7 });
    controller.writeDraft({ body: "ship this", attachments: [{ path: "uploads/a.png", name: "a.png" }] });

    const submission = controller.captureSubmission();
    repository.retarget(newCall);
    repository.controller(address({ agentId: "agent-2" })).writeDraft({ body: "other" });
    await controller.post(submission);

    expect(oldCall).toHaveBeenCalledWith("thread.post", expect.objectContaining({
      operation_id: "operation-1",
      entity_id: "run-1",
      agent_id: "agent-1",
      conversation_id: "conversation-1",
      choice_revision: 7,
      body: "ship this",
      attachments: [{ path: "uploads/a.png", name: "a.png" }],
    }));
    expect(newCall).not.toHaveBeenCalled();
  });

  it("restores the original composition after rejection only when no newer draft exists", () => {
    const controller = createRepository().controller(address());
    controller.writeDraft({ body: "original", attachments: [{ path: "one" }] });
    const submission = controller.captureSubmission();

    expect(controller.restoreRejected(submission, new Error("refused"))).toBe("restored");
    expect(controller.readDraft()).toMatchObject({ body: "original", attachments: [{ path: "one" }] });
    expect(controller.retryableFailures()).toEqual([]);
  });

  it("does not overwrite a newer draft and retains the rejected entry for retry", () => {
    const controller = createRepository().controller(address());
    controller.writeDraft({ body: "original", attachments: [{ path: "one" }] });
    const submission = controller.captureSubmission();
    controller.writeDraft({ body: "newer", attachments: [{ path: "two" }] });

    expect(controller.restoreRejected(submission, new Error("refused"))).toBe("retained");
    expect(controller.readDraft()).toMatchObject({ body: "newer", attachments: [{ path: "two" }] });
    expect(controller.retryableFailures()).toMatchObject([{
      operationId: "operation-1",
      message: { body: "original", attachments: [{ path: "one" }] },
      error: "refused",
    }]);
  });

  it("validates that a receipt belongs to the captured destination", async () => {
    const repository = createRepository(vi.fn(async (_method, params) => ({
      operation_id: params.operation_id,
      entity_id: params.entity_id,
      agent_id: "agent-2",
      posted_sequence: 2,
      operation_status: "queued",
    })));
    const controller = repository.controller(address());
    const submission = controller.captureSubmission({ body: "hello", attachments: [] });

    await expect(controller.post(submission)).rejects.toThrow("different agent");
  });

  it("treats a malformed v1 receipt as uncertain instead of a rejected draft", async () => {
    const repository = createRepository(vi.fn(async (_method, params) => ({
      operation_id: params.operation_id,
      posted_sequence: 2,
      operation_status: "queued",
    })));
    repository.configureCapabilities({ thread_post_operations: { version: 1, status_method: "thread.operation" } });
    const controller = repository.controller(address());
    const submission = controller.captureSubmission({ body: "hello", attachments: [] });

    await expect(controller.post(submission)).rejects.toMatchObject({ uncertain: true });
    expect(controller.readDraft().body).toBe("");
  });

  it("prevents completions from a disposed account/device scope changing controller state", async () => {
    let release;
    const held = new Promise((resolve) => { release = resolve; });
    const repository = createRepository(vi.fn(async (_method, params) => {
      await held;
      return {
        operation_id: params.operation_id,
        entity_id: params.entity_id,
        agent_id: params.agent_id,
        posted_sequence: 4,
        operation_status: "queued",
      };
    }));
    const controller = repository.controller(address());
    const submission = controller.captureSubmission({ body: "old scope", attachments: [] });
    const posting = controller.post(submission);
    repository.dispose();
    release();

    await expect(posting).rejects.toThrow("scope is no longer active");
  });

  it("uses a same-scope reconnect only for explicit retry and status reconciliation", async () => {
    const oldCall = vi.fn(async () => {
      throw Object.assign(new Error("timeout"), { timedOut: true, uncertain: true });
    });
    const newCall = vi.fn(async (method, params) => method === "thread.operation"
      ? {
          operation_id: params.operation_id,
          entity_id: params.entity_id,
          agent_id: params.agent_id,
          conversation_id: "conversation-1",
          choice_revision: 0,
          posted_sequence: 4,
          status: "queued",
        }
      : {
          operation_id: params.operation_id,
          entity_id: params.entity_id,
          agent_id: params.agent_id,
          conversation_id: "conversation-1",
          choice_revision: 0,
          posted_sequence: 4,
          operation_status: "queued",
        });
    const repository = createRepository(oldCall);
    repository.configureCapabilities({ thread_post_operations: { version: 1, status_method: "thread.operation" } });
    const controller = repository.controller(address());
    const submission = controller.captureSubmission({ body: "retry me", attachments: [] });

    await expect(controller.post(submission)).rejects.toThrow("timeout");
    repository.retarget(newCall);
    await expect(controller.operationStatus(submission)).resolves.toMatchObject({ status: "queued" });
    await expect(controller.retry(submission)).resolves.toMatchObject({ operation_id: "operation-1" });
    expect(newCall).toHaveBeenLastCalledWith("thread.post", expect.objectContaining({
      operation_id: "operation-1", agent_id: "agent-1", body: "retry me",
    }));
  });

  it("consumes the shared bridge operation fixture", () => {
    expect(operationContract).toMatchObject({
      version: 1,
      post_method: "thread.post",
      status_method: "thread.operation",
      statuses: ["queued", "claimed", "delivered", "uncertain"],
    });
    expect(operationContract.post_receipt_fields).toContain("conversation_id");
    expect(operationContract.status_receipt_fields).toContain("status");
  });

  it("prunes accepted operations instead of retaining a shadow copy of history", async () => {
    const repository = createRepository(vi.fn(async (_method, params) => ({
      operation_id: params.operation_id,
      entity_id: params.entity_id,
      agent_id: params.agent_id,
      conversation_id: "conversation-1",
      choice_revision: 0,
      posted_sequence: 4,
      operation_status: "delivered",
    })));
    repository.configureCapabilities({ thread_post_operations: { version: 1, status_method: "thread.operation" } });
    const controller = repository.controller(address());

    await controller.post(controller.captureSubmission({ body: "done" }));

    expect(controller.recoveries()).toEqual([]);
  });

  it("guards attachment settlement from an older remount after a newer draft", () => {
    const controller = createRepository().controller(address());
    const firstMount = controller.bindDraft();
    firstMount.writeAttachments([{ name: "old", status: "uploading" }]);
    const remount = controller.bindDraft();

    expect(remount.writeAttachments([{ name: "new", status: "ready" }])).toBe(true);
    expect(firstMount.writeAttachments([{ name: "old", status: "ready" }])).toBe(false);
    expect(controller.readAttachments()).toEqual([{ name: "new", status: "ready" }]);
  });
});

describe("per-agent model choices", () => {
  it("keeps two agents on the same provider independently configured", () => {
    const repository = createRepository();
    const first = repository.controller(address());
    const second = repository.controller(address({ agentId: "agent-2", conversationId: "conversation-2" }));

    first.absorbAgent({ provider: "codex", model: "gpt-5", effort: "high", choice_revision: 2 });
    second.absorbAgent({ provider: "codex", model: "gpt-5-mini", effort: "low", choice_revision: 9 });

    expect(first.choice()).toMatchObject({ requestedModel: "gpt-5", effort: "high", revision: 2 });
    expect(second.choice()).toMatchObject({ requestedModel: "gpt-5-mini", effort: "low", revision: 9 });
  });

  it("does not let an older snapshot retire a newer local model intent", () => {
    const controller = createRepository().controller(address());
    controller.absorbAgent({ model: "old", effort: "", active_model: "running", choice_revision: 4 });
    const intent = controller.requestChoice({ model: "new", effort: "high" });

    controller.absorbAgent({ model: "old", effort: "", active_model: "running", choice_revision: 4 });

    expect(controller.choice()).toMatchObject({
      requestedModel: "new",
      activeModel: "running",
      effort: "high",
      revision: 4,
      pending: true,
    });
    expect(intent.expectedRevision).toBe(4);
  });

  it("ignores out-of-order choice acknowledgements so the latest intent wins", () => {
    const controller = createRepository().controller(address());
    controller.absorbAgent({ model: "base", effort: "", choice_revision: 1 });
    const first = controller.requestChoice({ model: "first", effort: "low" });
    const second = controller.requestChoice({ model: "second", effort: "high" });

    controller.acknowledgeChoice(second, { agent_id: "agent-1", model: "second", effort: "high", choice_revision: 3 });
    controller.acknowledgeChoice(first, { agent_id: "agent-1", model: "first", effort: "low", choice_revision: 2 });

    expect(controller.choice()).toMatchObject({ requestedModel: "second", effort: "high", revision: 3, pending: false });
  });

  it("does not let a stale matching acknowledgement roll back a newer authoritative digest", () => {
    const controller = createRepository().controller(address());
    controller.absorbAgent({ id: "agent-1", model: "base", effort: "", choice_revision: 1 });
    const intent = controller.requestChoice({ model: "requested", effort: "high" });
    controller.absorbAgent({ id: "agent-1", model: "authoritative", effort: "low", choice_revision: 3 });

    controller.acknowledgeChoice(intent, {
      agent_id: "agent-1", model: "requested", effort: "high", choice_revision: 2,
    });

    expect(controller.choice()).toMatchObject({
      requestedModel: "authoritative", effort: "low", revision: 3, pending: false,
    });
  });
});
