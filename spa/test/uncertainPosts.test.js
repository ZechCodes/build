// #30 point 2: after a reconnect, a "Delivery uncertain" post resolves itself.
//
// The maintainer's post reached the bridge and its receipt did not reach them,
// so the composer said "Delivery uncertain … Check delivery" and waited for a
// press. The answer was already available: `thread.operation` is the bridge's
// durable ledger of every post it admitted, and it says one of three things —
//
//   a receipt          it landed; the post is Sent and the strip clears
//   status "uncertain" the bridge HAS it and does not know whether the provider
//                      took it; re-sending would duplicate the message
//   not_found          it never arrived; this is the one that is re-sent
//
// — which is why the re-send is safe to do without asking. It happens once per
// post, in submission order, because a conversation is a sequence.

import { describe, expect, it, vi } from "vitest";
import { createChatRepository } from "../src/core/chatRepository.js";

const address = (over = {}) => ({
  entityId: "run-1",
  agentId: "agent-1",
  conversationId: "conversation-1",
  ...over,
});

const OPERATIONS = { api_version: "1.19.0", thread_post_operations: { version: 1, status_method: "thread.operation" } };

/** The bridge's post receipt for an operation it admitted. */
const postReceipt = (params, status = "queued") => ({
  operation_id: params.operation_id,
  entity_id: params.entity_id,
  agent_id: params.agent_id,
  conversation_id: params.conversation_id || "conversation-1",
  choice_revision: 0,
  posted_sequence: 4,
  operation_status: status,
});

/** Its ledger answer for the same operation. */
const statusReceipt = (params, status = "queued") => ({
  operation_id: params.operation_id,
  entity_id: params.entity_id,
  agent_id: params.agent_id,
  conversation_id: "conversation-1",
  choice_revision: 0,
  posted_sequence: 4,
  status,
});

/** What the bridge says about an operation it never received: the refusal
 *  `AppState::thread_operation` writes, classified `not_found` on the way out
 *  (bridge/src/api/mod.rs). */
const unknownOperation = (operationId) =>
  Object.assign(new Error(`unknown operation_id: ${operationId}`), { code: "not_found", error_code: "not_found" });

/** A repository with a post already stranded uncertain: the send went out on a
 *  wire that then stopped carrying, exactly as the maintainer's did. */
async function stranded({ bodies = ["first"], statusCall } = {}) {
  const deadWire = vi.fn(async () => {
    throw Object.assign(new Error("thread.post timed out"), { timedOut: true, uncertain: true, deadline: "path" });
  });
  const repository = createChatRepository({
    scope: { accountId: "account-1", deviceId: "device-1" },
    call: deadWire,
    createOperationId: (() => {
      let n = 0;
      return () => `operation-${++n}`;
    })(),
  });
  repository.configureCapabilities(OPERATIONS);
  const controller = repository.controller(address());
  const submissions = [];
  for (const body of bodies) {
    const submission = controller.captureSubmission({ body, attachments: [] });
    submissions.push(submission);
    await expect(controller.post(submission)).rejects.toThrow("timed out");
  }
  expect(controller.recoveries().map((one) => one.status)).toEqual(bodies.map(() => "uncertain"));
  const reconnected = vi.fn(statusCall);
  return { repository, controller, submissions, reconnected, reconnect: () => repository.retarget(reconnected) };
}

describe("resolving uncertain posts after a reconnect", () => {
  it("shows a post that landed as sent, with no press", async () => {
    const { repository, controller, reconnect, reconnected } = await stranded({
      statusCall: async (method, params) => {
        expect(method).toBe("thread.operation");
        return statusReceipt(params, "delivered");
      },
    });
    reconnect();

    const resolved = await repository.resolveUncertainPosts();

    expect(resolved).toMatchObject([{ operationId: "operation-1", outcome: "landed", status: "delivered" }]);
    expect(controller.recoveries()).toEqual([]);
    expect(reconnected).toHaveBeenCalledTimes(1); // asked, not re-sent
  });

  it("re-sends a post the bridge never received", async () => {
    const { repository, controller, reconnect, reconnected } = await stranded({
      statusCall: async (method, params) => {
        if (method === "thread.operation") throw unknownOperation(params.operation_id);
        return postReceipt(params, "queued");
      },
    });
    reconnect();

    const resolved = await repository.resolveUncertainPosts();

    expect(resolved).toMatchObject([{ operationId: "operation-1", outcome: "resent" }]);
    expect(reconnected.mock.calls.map(([method]) => method)).toEqual(["thread.operation", "thread.post"]);
    // The same operation id, so a bridge that did have it after all dedupes
    // rather than writing the message twice.
    expect(reconnected).toHaveBeenLastCalledWith("thread.post", expect.objectContaining({
      operation_id: "operation-1",
      body: "first",
    }));
    expect(controller.recoveries()).toEqual([]);
  });

  it("leaves a post the bridge is itself unsure about alone — re-sending would double it", async () => {
    const { repository, controller, reconnect, reconnected } = await stranded({
      statusCall: async (_method, params) => statusReceipt(params, "uncertain"),
    });
    reconnect();

    const resolved = await repository.resolveUncertainPosts();

    expect(resolved).toMatchObject([{ operationId: "operation-1", outcome: "uncertain-at-bridge" }]);
    expect(reconnected.mock.calls.map(([method]) => method)).toEqual(["thread.operation"]);
    expect(controller.recoveries().map((one) => one.status)).toEqual(["uncertain"]);
  });

  it("re-sends in submission order: a conversation is a sequence", async () => {
    const sent = [];
    const { repository, reconnect } = await stranded({
      bodies: ["first", "second", "third"],
      statusCall: async (method, params) => {
        if (method === "thread.operation") throw unknownOperation(params.operation_id);
        sent.push(params.body);
        return postReceipt(params, "queued");
      },
    });
    reconnect();

    await repository.resolveUncertainPosts();

    expect(sent).toEqual(["first", "second", "third"]);
  });

  it("re-sends once: a second reconnect does not send the message again", async () => {
    const { repository, reconnect, reconnected } = await stranded({
      statusCall: async (method, params) => {
        if (method === "thread.operation") throw unknownOperation(params.operation_id);
        // The re-send dies on the wire too, so the post is still uncertain.
        throw Object.assign(new Error("thread.post timed out"), { timedOut: true, uncertain: true });
      },
    });
    reconnect();

    await repository.resolveUncertainPosts();
    await repository.resolveUncertainPosts();

    expect(reconnected.mock.calls.map(([method]) => method)).toEqual([
      "thread.operation", "thread.post", "thread.operation",
    ]);
  });

  it("keeps an unanswerable post uncertain for the next reconnect", async () => {
    const { repository, controller, reconnect } = await stranded({
      statusCall: async () => {
        throw Object.assign(new Error("thread.operation timed out"), { timedOut: true, deadline: "path" });
      },
    });
    reconnect();

    const resolved = await repository.resolveUncertainPosts();

    expect(resolved).toMatchObject([{ operationId: "operation-1", outcome: "unresolved" }]);
    expect(controller.recoveries().map((one) => one.status)).toEqual(["uncertain"]);
  });

  it("resolves every conversation's posts, not only the one on screen", async () => {
    const asked = [];
    const deadWire = vi.fn(async () => {
      throw Object.assign(new Error("thread.post timed out"), { timedOut: true, uncertain: true });
    });
    let n = 0;
    const repository = createChatRepository({
      scope: { accountId: "account-1", deviceId: "device-1" },
      call: deadWire,
      createOperationId: () => `operation-${++n}`,
    });
    repository.configureCapabilities(OPERATIONS);
    for (const agentId of ["agent-1", "agent-2"]) {
      const controller = repository.controller(address({ agentId, conversationId: `conversation-${agentId}` }));
      const submission = controller.captureSubmission({ body: `for ${agentId}`, attachments: [] });
      await expect(controller.post(submission)).rejects.toThrow("timed out");
    }
    repository.retarget(async (method, params) => {
      asked.push(params.agent_id);
      // thread.operation is addressed by operation id; the conversation comes
      // back on the receipt, and the client checks it is the one it asked about.
      return { ...statusReceipt(params, "delivered"), conversation_id: `conversation-${params.agent_id}` };
    });

    const resolved = await repository.resolveUncertainPosts();

    expect(asked).toEqual(["agent-1", "agent-2"]);
    expect(resolved.map((one) => one.outcome)).toEqual(["landed", "landed"]);
  });

  it("says nothing to a bridge with no operation ledger to ask", async () => {
    const deadWire = vi.fn(async () => {
      throw Object.assign(new Error("thread.post timed out"), { timedOut: true, uncertain: true });
    });
    const repository = createChatRepository({
      scope: { accountId: "account-1", deviceId: "device-1" },
      call: deadWire,
      createOperationId: () => "operation-1",
    });
    const controller = repository.controller(address());
    const submission = controller.captureSubmission({ body: "no ledger", attachments: [] });
    await expect(controller.post(submission)).rejects.toThrow("timed out");
    const reconnected = vi.fn();
    repository.retarget(reconnected);

    expect(await repository.resolveUncertainPosts()).toEqual([]);
    expect(reconnected).not.toHaveBeenCalled();
    expect(controller.recoveries().map((one) => one.status)).toEqual(["uncertain"]);
  });

  it("is quiet on a repository whose scope has gone", async () => {
    const { repository, reconnect } = await stranded({ statusCall: async () => ({}) });
    reconnect();
    repository.dispose();

    expect(await repository.resolveUncertainPosts()).toEqual([]);
  });
});
