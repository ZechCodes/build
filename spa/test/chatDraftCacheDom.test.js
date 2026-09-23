// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

it("announces a typed draft after its cache write can be read back", async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  const { createChatRepository } = await import("../src/core/chatRepository.js");
  const { readCached } = await import("../src/core/localCache.js");
  const identity = { entityId: "run-1", agentId: "agent-1", conversationId: "conversation-1" };
  const repository = createChatRepository({ scope: { deviceId: "dev-1" }, call: vi.fn() });
  const controller = repository.controller(identity);
  const observed = [];
  controller.subscribe(() => observed.push(controller.readDraft().body));
  controller.writeDraft({ body: "cached words" });
  expect(observed).toEqual([]);
  await vi.waitFor(() => expect(observed).toContain("cached words"));
  expect((await readCached(repository.draftAddress(identity))).value.body).toBe("cached words");
  controller.dispose();
});
