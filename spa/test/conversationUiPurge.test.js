// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const SCOPE = {
  deviceId: "dev-1", entityId: "workspace-1", agentId: "agent-1",
  conversationId: "agent-1", threadId: "thread:old",
};
const DRAFT = {
  deviceId: "dev-1", entityId: "agent-1", kind: "ui-draft",
  sub: "chat:agent:workspace-1:agent-1:thread:old",
};
const journalKey = (address) => `build.ui.pending:${JSON.stringify([
  address.deviceId, address.entityId, address.kind, address.sub,
])}`;
let ui, store;

class TestChannel {
  static opened = [];
  constructor(name) { this.name = name; TestChannel.opened.push(this); }
  postMessage(data) {
    for (const channel of TestChannel.opened) {
      if (channel !== this && channel.name === this.name) queueMicrotask(() => channel.onmessage?.({ data }));
    }
  }
  unref() {}
}

async function reload() {
  vi.resetModules();
  store = await import("../src/core/localUiStore.js");
  ui = await import("../src/core/localUiState.js");
}

beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  vi.stubGlobal("BroadcastChannel", TestChannel);
  TestChannel.opened = [];
  sessionStorage.clear();
  await reload();
});

describe("clearing one conversation's local UI records", () => {
  it("removes only the old conversation's drafts and transient records, preserving preferences and siblings", async () => {
    const old = [
      DRAFT,
      { ...DRAFT, sub: "chat:agent:workspace-1:agent-1" },
      { deviceId: "dev-1", entityId: "workspace-1", kind: "ui-fold", sub: "thread:agent-1:thread:old" },
      { deviceId: "dev-1", entityId: "workspace-1", kind: "ui-fold", sub: "thread:agent-1" },
      { deviceId: "", entityId: "workspace-1:agent-1:thread:old", kind: "ui-menu", sub: "agent-surfaces:" },
      { deviceId: "", entityId: "workspace-1:agent-1", kind: "ui-fold", sub: "surface-viewer:tasks" },
      { deviceId: "", entityId: "workspace-1:agent-1", kind: "ui-menu", sub: "composer:model" },
    ];
    const kept = [
      { ...DRAFT, sub: "chat:agent:workspace-1:agent-1:thread:new" },
      { ...DRAFT, deviceId: "dev-2" },
      { ...DRAFT, entityId: "agent-2", sub: "chat:agent:workspace-1:agent-2:thread:old" },
      { deviceId: "dev-1", entityId: "workspace-1", kind: "ui-filter", sub: "thread:agent-1" },
      { deviceId: "dev-1", entityId: "workspace-1", kind: "ui-fold", sub: "thread:agent-2" },
      { deviceId: "", entityId: "workspace-1:agent-2", kind: "ui-menu", sub: "agent-surfaces:" },
      { deviceId: "", entityId: "", kind: "ui-fold", sub: "agent-rail:pinned" },
    ];
    for (const address of [...old, ...kept]) await store.writeUiRecord(address, { body: address.sub });
    await ui.purgeConversationUiRecords(SCOPE);
    for (const address of old) expect(await store.readUiRecord(address)).toBeUndefined();
    for (const address of kept) expect((await store.readUiRecord(address))?.value?.body).toBe(address.sub);
  });

  it("discards scheduled edits and the exit journal before they can recreate a cleared draft", async () => {
    const watcher = ui.watchUiState(DRAFT, () => {}, { debounceMs: 60_000 });
    await watcher.ready;
    watcher.schedule({ body: "discard this unfinished text" });
    sessionStorage.setItem(journalKey(DRAFT), JSON.stringify({
      at: Date.now(), source: "other-tab", sequence: 1, value: { body: "old exit journal" },
    }));
    await ui.purgeConversationUiRecords(SCOPE);
    await watcher.flush();
    window.dispatchEvent(new Event("pagehide"));
    expect(sessionStorage.getItem(journalKey(DRAFT))).toBeNull();
    expect(await store.readUiRecord(DRAFT)).toBeUndefined();
    watcher.dispose();
  });

  it("keeps retired records unwritable across reload and journal replay", async () => {
    await store.writeUiRecord(DRAFT, { body: "old" });
    await ui.purgeConversationUiRecords(SCOPE);
    await reload();
    await store.writeUiRecord(DRAFT, { body: "late" });
    sessionStorage.setItem(journalKey(DRAFT), JSON.stringify({
      at: Date.now() + 1000, source: "old-tab", sequence: 2, value: { body: "late journal" },
    }));
    const watcher = ui.watchUiState(DRAFT, () => {}, { debounceMs: 60_000 });
    await watcher.ready;
    expect(await store.readUiRecord(DRAFT)).toBeUndefined();
    expect(sessionStorage.getItem(journalKey(DRAFT))).toBeNull();
    const fresh = { ...DRAFT, sub: "chat:agent:workspace-1:agent-1:thread:new" };
    await store.writeUiRecord(fresh, { body: "fresh" });
    expect((await store.readUiRecord(fresh))?.value?.body).toBe("fresh");
    watcher.dispose({ flushPending: false });
  });

  it("retires a writer in another tab and prevents its delayed commit", async () => {
    const firstUi = ui;
    const firstStore = store;
    const watcher = firstUi.watchUiState(DRAFT, () => {}, { debounceMs: 60_000 });
    await watcher.ready;
    watcher.schedule({ body: "old-tab text" });
    await reload();
    await ui.purgeConversationUiRecords(SCOPE);
    await new Promise((resolve) => setTimeout(resolve, 0));
    await watcher.write({ body: "another late write" });
    window.dispatchEvent(new Event("pagehide"));
    expect(sessionStorage.getItem(journalKey(DRAFT))).toBeNull();
    expect(await firstStore.readUiRecord(DRAFT)).toBeUndefined();
    watcher.dispose({ flushPending: false });
  });

  it("refuses retired replica drafts and delayed conditional writes", async () => {
    await store.writeUiRecord(DRAFT, { body: "before clear" });
    const captured = await store.readUiRecord(DRAFT);
    await ui.purgeConversationUiRecords(SCOPE);
    expect(await store.writeUiRecordIfUnwritten(DRAFT, undefined, { body: "late clear" })).toBe(false);
    expect(await store.writeUiRecordIfNewer(DRAFT, { body: "late journal" }, {
      at: captured.at + 1000, source: "old-tab", sequence: 1,
    })).toBe(false);
    const cache = await import("../src/core/localCache.js");
    await cache.writeCached(DRAFT, { body: "an older tab's replica draft" });
    await store.adoptCachedUiRecords();
    expect(await store.readUiRecord(DRAFT)).toBeUndefined();
    expect(await cache.readCached(DRAFT)).toBeUndefined();
  });

  it("leaves a different conversation's scheduled draft and exit journal usable", async () => {
    const sibling = { ...DRAFT, entityId: "agent-2", sub: "chat:agent:workspace-1:agent-2:thread:old" };
    const watcher = ui.watchUiState(sibling, () => {}, { debounceMs: 60_000 });
    await watcher.ready;
    watcher.schedule({ body: "keep this unfinished thought" });
    sessionStorage.setItem(journalKey(sibling), JSON.stringify({
      at: Date.now(), source: "other-tab", sequence: 1, value: { body: "keep the journal" },
    }));
    await ui.purgeConversationUiRecords(SCOPE);
    expect(sessionStorage.getItem(journalKey(sibling))).toContain("keep the journal");
    await watcher.flush();
    expect((await store.readUiRecord(sibling))?.value?.body).toBe("keep this unfinished thought");
    watcher.dispose();
  });
});
