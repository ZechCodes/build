// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as revisions from "../src/core/revisionBodies.js";
import * as lightboxes from "../src/core/threadAttachmentLightbox.js";

const ownership = { deviceId: "device-1", entityId: "run-1", agentId: "secondary-agent", conversationId: "agent-1", threadId: "old-thread" };
const other = { ...ownership, deviceId: "device-2" };
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
const flush = async () => { for (let step = 0; step < 5; step += 1) await Promise.resolve(); };
const preview = (name, source) => {
  const trigger = document.createElement("button");
  trigger.textContent = name;
  document.body.append(trigger);
  return { trigger, name, path: name, kind: "image", source };
};

beforeEach(() => {
  document.body.replaceChildren();
  revisions.forgetRevisionBodies();
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  URL.createObjectURL = vi.fn(() => "blob:cleared-preview");
  URL.revokeObjectURL = vi.fn();
});
afterEach(async () => {
  lightboxes.closeConversationAttachmentLightboxes?.({});
  await flush();
  revisions.forgetRevisionBodies();
  vi.unstubAllGlobals();
});

describe("cleared conversation content", () => {
  it("forgets cached revisions only in the cleared device and conversation", async () => {
    const call = vi.fn(async () => ({ contents: "old revision" }));
    const otherCall = vi.fn(async () => ({ contents: "other device revision" }));
    await revisions.revisionContents("run-1", "rev-1", call, ownership);
    await revisions.revisionContents("run-1", "rev-1", otherCall, other);
    revisions.forgetConversationRevisionBodies(ownership);
    call.mockResolvedValue({ contents: "fresh revision" });
    expect(await revisions.revisionContents("run-1", "rev-1", call, ownership)).toEqual({ contents: "fresh revision" });
    expect(await revisions.revisionContents("run-1", "rev-1", otherCall, other)).toEqual({ contents: "other device revision" });
    expect(call).toHaveBeenCalledTimes(2);
    expect(otherCall).toHaveBeenCalledTimes(1);
  });

  it("rejects a revision read that finishes after its conversation is cleared", async () => {
    const pending = deferred();
    const reading = revisions.revisionContents("run-1", "rev-1", () => pending.promise, ownership);
    const refused = expect(reading).rejects.toThrow(/cleared/);
    revisions.forgetConversationRevisionBodies(ownership);
    pending.resolve({ contents: "late old revision" });
    await refused;
  });

  it("passes the exact conversation generation to the revision read", async () => {
    const call = vi.fn(async () => ({ contents: "revision" }));
    await revisions.revisionContents("run-1", "rev-1", call, ownership);
    expect(call).toHaveBeenCalledWith("thread.revision", {
      entity_id: "run-1", revision_id: "rev-1", agent_id: "secondary-agent", conversation_id: "agent-1", thread_id: "old-thread",
    });
  });

  it("closes and releases cleared previews while keeping another device's preview", async () => {
    const source = async () => ({ pages: ["aGVsbG8="], mime: "image/png" });
    const closed = lightboxes.openAttachmentLightbox([preview("old image", source)], 0, ownership);
    const retained = lightboxes.openAttachmentLightbox([preview("other image", source)], 0, other);
    await flush();
    lightboxes.closeConversationAttachmentLightboxes(ownership);
    expect(document.body.contains(closed.body)).toBe(false);
    expect(document.body.contains(retained.body)).toBe(true);
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
  });

  it("never displays a late attachment after reset closes its preview", async () => {
    const pending = deferred();
    const modal = lightboxes.openAttachmentLightbox([preview("late image", () => pending.promise)], 0, ownership);
    await flush();
    lightboxes.closeConversationAttachmentLightboxes(ownership);
    pending.resolve({ pages: ["aGVsbG8="], mime: "image/png" });
    await flush();
    expect(document.body.contains(modal.body)).toBe(false);
    expect(URL.createObjectURL).not.toHaveBeenCalled();
    expect(modal.body.querySelector("img")).toBe(null);
  });
});
