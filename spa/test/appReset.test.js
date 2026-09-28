// @vitest-environment jsdom
// Putting the application back to its just-loaded state.
//
// Everything a device's surfaces held is let go — its transport, its
// terminals, its cache scope — and, since the cache is what the app paints
// from, what it wrote to disk goes with them. An embedder that replaces the
// account must not leave the previous one's board, conversations and diffs
// readable in the browser profile.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: () => () => {},
  refreshFeed: async () => [],
  dropFeedDevice: () => {},
  startFeed: () => {},
  stopFeed: () => {},
}));
vi.mock("../src/core/inboxView.js", () => ({ markSeen: async () => {} }));
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose() {} }) }));

const { resetApplication } = await import("../src/app.js");
const { readCached, writeCached } = await import("../src/core/localCache.js");
const { readUiRecord, writeUiRecord } = await import("../src/core/localUiStore.js");

const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(async () => {
  document.body.innerHTML = "";
});

describe("resetApplication", () => {
  it("wipes what the previous account's devices cached on this browser", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "ws-1", kind: "status" }, { head: "theirs" });
    await writeCached({ deviceId: "dev-2", entityId: "", kind: "feed" }, { items: [{ id: "one" }] });

    resetApplication();
    await settled();

    expect(await readCached({ deviceId: "dev-1", entityId: "ws-1", kind: "status" })).toBeUndefined();
    expect(await readCached({ deviceId: "dev-2", entityId: "", kind: "feed" })).toBeUndefined();
  });

  it("wipes the previous account's unsent drafts and UI state too", async () => {
    const draft = { deviceId: "dev-1", entityId: "conv-1", kind: "ui-draft", sub: "chat:" };
    await writeUiRecord(draft, { body: "theirs" });

    resetApplication();
    await vi.waitFor(async () => expect(await readUiRecord(draft)).toBeUndefined());
  });
});
