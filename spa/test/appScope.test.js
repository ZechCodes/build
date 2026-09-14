// @vitest-environment jsdom
// The home device's chat repository, as the surfaces that still speak in scopes
// see it. What a machine holds that is the machine's own — its harness catalog
// among it — is deviceContexts.test.js's.
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App, adoptApplicationScope, disposeApplicationScope } from "../src/app.js";

describe("the application-owned chat repository", () => {
  beforeEach(() => disposeApplicationScope());

  it("retargets the same device without losing its controllers or drafts", () => {
    const firstCall = vi.fn();
    const nextCall = vi.fn();
    const repository = adoptApplicationScope({ deviceId: "device-a", call: firstCall });
    const controller = repository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "thread-1",
    });
    controller.writeDraft({ body: "keep this" });

    const resumed = adoptApplicationScope({ deviceId: "device-a", call: nextCall });

    expect(resumed).toBe(repository);
    expect(resumed.controller(controller.identity)).toBe(controller);
    expect(controller.readDraft().body).toBe("keep this");
    expect(resumed.currentCall()).toBe(nextCall);
  });

  it("retires the old repository before another device is adopted", () => {
    const oldRepository = adoptApplicationScope({ deviceId: "device-a", call: vi.fn() });
    const oldController = oldRepository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "thread-1",
    });

    const nextRepository = adoptApplicationScope({ deviceId: "device-b", call: vi.fn() });

    expect(nextRepository).not.toBe(oldRepository);
    expect(App.chatRepository).toBe(nextRepository);
    expect(App.cacheScope.deviceId).toBe("device-b");
    expect(() => oldController.writeDraft({ body: "too late" })).toThrow(/no longer active/);
    expect(oldController.threadState.active()).toBe(false);
  });

  it("captures the old transport for a send accepted before reconnect", async () => {
    let release;
    const oldCall = vi.fn(() => new Promise((resolve) => { release = resolve; }));
    const nextCall = vi.fn();
    const repository = adoptApplicationScope({ deviceId: "device-a", call: oldCall });
    const controller = repository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "thread-1",
    });
    controller.writeDraft({ body: "captured" });
    const submission = controller.captureSubmission();
    const pending = controller.post(submission);

    adoptApplicationScope({ deviceId: "device-a", call: nextCall });
    release({ operation_id: submission.operationId });
    await pending;

    expect(oldCall).toHaveBeenCalledTimes(1);
    expect(nextCall).not.toHaveBeenCalled();
  });
});
