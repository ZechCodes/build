// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const localUiStateModule = "../src/core/localUiState.js";

afterEach(() => vi.doUnmock(localUiStateModule));

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

function draftWriterHarness() {
  let paint = () => {};
  let pending;
  let persisted;
  const flushes = [];
  const watchUiState = vi.fn((_address, nextPaint) => {
    paint = nextPaint;
    return {
      ready: Promise.resolve(),
      schedule(value) { pending = value; },
      flush: vi.fn(() => {
        const value = pending;
        pending = undefined;
        const gate = deferred();
        const flush = {
          value,
          release() {
            if (value !== undefined) persisted = value;
            // A superseded write commits but does not read back or paint while
            // a newer edit is pending.
            if (value !== undefined && pending === undefined) paint(value);
            gate.resolve();
          },
          reject(error = new Error("draft cache unavailable")) { gate.reject(error); },
        };
        flushes.push(flush);
        return gate.promise;
      }),
      write: vi.fn(),
      dispose: vi.fn(),
    };
  });
  return { watchUiState, flushes, persisted: () => persisted };
}

const uiAddress = ({ deviceId = "", entityId = "", view, kind, sub = "" }) => ({
  deviceId, entityId, kind: `ui-${kind}`, sub: `${view}:${sub}`,
});

async function repositoryWithWriter(writer) {
  vi.resetModules();
  vi.doMock(localUiStateModule, () => ({ uiAddress, watchUiState: writer.watchUiState }));
  const { createChatRepository } = await import("../src/core/chatRepository.js");
  const repository = createChatRepository({ scope: {}, call: vi.fn() });
  const identity = { entityId: "run-1", agentId: "agent-1", conversationId: "conversation-1" };
  return { repository, controller: repository.controller(identity) };
}

const greeting = { api_version: "3.36.0", capabilities: [] };

it("announces a typed draft after its cache write can be read back", async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  const { createChatRepository } = await import("../src/core/chatRepository.js");
  const { readUiRecord } = await import("../src/core/localUiStore.js");
  const identity = { entityId: "run-1", agentId: "agent-1", conversationId: "conversation-1" };
  const repository = createChatRepository({ scope: { deviceId: "dev-1" }, call: vi.fn() });
  const controller = repository.controller(identity);
  const observed = [];
  controller.subscribe(() => observed.push(controller.readDraft().body));
  controller.writeDraft({ body: "cached words" });
  expect(observed).toEqual([]);
  await vi.waitFor(() => expect(observed).toContain("cached words"));
  expect((await readUiRecord(repository.draftAddress(identity))).value.body).toBe("cached words");
  controller.dispose();
});

it("waits for a newer draft scheduled during a capability flush", async () => {
  const writer = draftWriterHarness();
  const { repository, controller } = await repositoryWithWriter(writer);
  const observed = [];
  controller.subscribe(() => observed.push(controller.readDraft().body));
  controller.writeDraft({ body: "first edit" });

  repository.configureCapabilities(greeting);
  expect(writer.flushes).toHaveLength(1);
  controller.writeDraft({ body: "newer edit" });
  writer.flushes[0].release();
  await Promise.resolve();
  await Promise.resolve();

  expect(observed).not.toContain("newer edit");
  expect(writer.flushes).toHaveLength(2);
  writer.flushes[1].release();
  await vi.waitFor(() => expect(observed).toContain("newer edit"));
  expect(writer.persisted().body).toBe("newer edit");
  controller.dispose();
});

it("waits for a draft scheduled after an idle capability flush starts", async () => {
  const writer = draftWriterHarness();
  const { repository, controller } = await repositoryWithWriter(writer);
  const observed = [];
  controller.subscribe(() => observed.push(controller.readDraft().body));

  repository.configureCapabilities(greeting);
  expect(writer.flushes).toHaveLength(1);
  controller.writeDraft({ body: "late edit" });
  writer.flushes[0].release();
  await Promise.resolve();
  await Promise.resolve();

  expect(observed).not.toContain("late edit");
  expect(writer.flushes).toHaveLength(2);
  writer.flushes[1].release();
  await vi.waitFor(() => expect(observed).toContain("late edit"));
  expect(writer.persisted().body).toBe("late edit");
  controller.dispose();
});

it("stays silent after a rejected flush and announces after a later committed edit", async () => {
  const writer = draftWriterHarness();
  const { repository, controller } = await repositoryWithWriter(writer);
  const observed = [];
  controller.subscribe(() => observed.push(controller.readDraft().body));
  controller.writeDraft({ body: "uncommitted edit" });

  repository.configureCapabilities(greeting);
  writer.flushes[0].reject();
  await Promise.resolve();
  await Promise.resolve();
  expect(observed).toEqual([]);

  controller.writeDraft({ body: "committed retry" });
  repository.configureCapabilities(greeting);
  expect(writer.flushes).toHaveLength(2);
  writer.flushes[1].release();
  await vi.waitFor(() => expect(observed).toContain("committed retry"));
  expect(writer.persisted().body).toBe("committed retry");
  controller.dispose();
});
