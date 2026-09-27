// @vitest-environment jsdom
// UI records use the real IndexedDB cache and its address announcements.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, ui, focus, menus, runs;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  sessionStorage.clear();
  document.body.innerHTML = "";
  cache = await import("../src/core/localCache.js");
  ui = await import("../src/core/localUiState.js");
  focus = await import("../src/core/focusMemory.js");
  menus = await import("../src/core/filterMenuControl.js");
  runs = await import("../src/core/activityRuns.js");
});

describe("local UI cache wiring", () => {
  it("mounts a draft from disk, then repaints only after a write is read back", async () => {
    const address = ui.uiAddress({ deviceId: "dev-1", entityId: "conversation-1", view: "chat", kind: "draft" });
    await cache.writeCached(address, { text: "before reload" });
    const painted = [];
    const record = ui.watchUiState(address, (value) => painted.push(value.text), { debounceMs: 5 });
    await record.ready;
    expect(painted).toEqual(["before reload"]);

    await cache.writeCached(address, { text: "another tab" });
    await vi.waitFor(() => expect(painted.at(-1)).toBe("another tab"));
    record.schedule({ text: "after typing" });
    await vi.waitFor(() => expect(painted.at(-1)).toBe("after typing"));
    expect((await cache.readCached(address)).value.text).toBe("after typing");
    record.dispose();
  });

  it("restores focus from the cached route record without stealing a later focus move", async () => {
    const root = document.createElement("main");
    root.innerHTML = '<button id="first">First</button><button id="second">Second</button>';
    document.body.append(root);
    const address = ui.uiAddress({ deviceId: "dev-1", entityId: "project-1", view: "project", kind: "focus" });
    await cache.writeCached(address, { selector: "button:nth-child(1)" });
    const dispose = focus.mountFocusMemory(root, "project", { deviceId: "dev-1", entityId: "project-1" });
    await vi.waitFor(() => expect(document.activeElement).toBe(root.querySelector("#first")));
    root.querySelector("#second").focus();
    await vi.waitFor(async () => expect((await cache.readCached(address)).value.selector).toBe("button:nth-child(2)"));
    await cache.writeCached(address, { selector: "button:nth-child(1)" });
    await dispose.settled();
    expect(document.activeElement).toBe(root.querySelector("#second"));
    dispose();
  });

  it("flushes a debounced draft on pagehide before a remount", async () => {
    const address = ui.uiAddress({ entityId: "conversation-reload", view: "chat", kind: "draft" });
    await cache.writeCached(address, { text: "older text" });
    const first = ui.watchUiState(address, () => {}, { debounceMs: 60_000 });
    await first.ready;
    first.schedule({ text: "last keystroke" });
    window.dispatchEvent(new Event("pagehide"));
    await vi.waitFor(async () => expect((await cache.readCached(address))?.value?.text).toBe("last keystroke"));
    first.dispose();

    const painted = [];
    const remounted = ui.watchUiState(address, (saved) => painted.push(saved.text));
    await remounted.ready;
    expect(painted).toEqual(["last keystroke"]);
    remounted.dispose();
  });

  it("discards a journal older than another tab's draft, and an old entry after send", async () => {
    const address = ui.uiAddress({ deviceId: "dev-1", entityId: "conversation-stale", view: "chat", kind: "draft" });
    const key = `build.ui.pending:${JSON.stringify([address.deviceId, address.entityId, address.kind, address.sub])}`;
    await cache.writeCached(address, { text: "first" });
    const first = ui.watchUiState(address, () => {}, { debounceMs: 60_000 });
    await first.ready;
    first.schedule({ text: "unfinished" });
    window.dispatchEvent(new Event("pagehide"));
    const journal = sessionStorage.getItem(key);
    expect(journal).toContain("unfinished");
    await first.flush();
    first.dispose({ flushPending: false });

    sessionStorage.setItem(key, journal);
    await cache.writeCached(address, { text: "newer tab" });
    const painted = [];
    const second = ui.watchUiState(address, (saved) => painted.push(saved.text), { debounceMs: 60_000 });
    await second.ready;
    expect(painted.at(-1)).toBe("newer tab");
    expect((await cache.readCached(address)).value.text).toBe("newer tab");
    expect(sessionStorage.getItem(key)).toBeNull();

    second.schedule({ text: "before send" });
    window.dispatchEvent(new Event("pagehide"));
    const beforeSend = sessionStorage.getItem(key);
    await second.write({ text: "" });
    second.dispose({ flushPending: false });
    sessionStorage.setItem(key, beforeSend);
    const afterSend = ui.watchUiState(address, (saved) => painted.push(saved.text), { debounceMs: 60_000 });
    await afterSend.ready;
    expect((await cache.readCached(address)).value.text).toBe("");
    expect(painted.at(-1)).toBe("");
    expect(sessionStorage.getItem(key)).toBeNull();
    afterSend.dispose();
  });

  it("drops a local edit superseded before readback, including on the next mount", async () => {
    const address = ui.uiAddress({ deviceId: "dev-1", entityId: "competing-draft", view: "chat", kind: "draft" });
    const key = `build.ui.pending:${JSON.stringify([address.deviceId, address.entityId, address.kind, address.sub])}`;
    let finishCompeting;
    const competing = new Promise((resolve) => { finishCompeting = resolve; });
    let raced = false;
    const stopCompeting = cache.subscribeCache(address, () => {
      if (raced) return;
      raced = true;
      void cache.writeCached(address, { text: "newer other writer" }).then(finishCompeting);
    });
    const painted = [];
    const first = ui.watchUiState(address, (saved) => painted.push(saved.text), { debounceMs: 60_000 });
    await first.ready;
    await first.write({ text: "old local writer" });
    await competing;
    await vi.waitFor(() => expect(painted.at(-1)).toBe("newer other writer"));
    expect(painted).not.toContain("old local writer");
    expect((await cache.readCached(address)).value.text).toBe("newer other writer");
    window.dispatchEvent(new Event("pagehide"));
    expect(sessionStorage.getItem(key)).toBeNull();
    first.dispose({ flushPending: false });
    stopCompeting();

    const reloaded = ui.watchUiState(address, (saved) => painted.push(saved.text), { debounceMs: 60_000 });
    await reloaded.ready;
    expect(painted.at(-1)).toBe("newer other writer");
    expect((await cache.readCached(address)).value.text).toBe("newer other writer");
    reloaded.dispose();
  });

  it("dates the exit journal when the edit happened, not when the page exited", async () => {
    const address = ui.uiAddress({ entityId: "edit-order", view: "chat", kind: "draft" });
    const key = `build.ui.pending:${JSON.stringify(["", address.entityId, address.kind, address.sub])}`;
    let clock = 10_000;
    const now = vi.spyOn(Date, "now").mockImplementation(() => clock);
    try {
      const record = ui.watchUiState(address, () => {}, { debounceMs: 60_000 });
      await record.ready;
      record.schedule({ text: "typed earlier" });
      clock = 20_000;
      window.dispatchEvent(new Event("pagehide"));
      expect(JSON.parse(sessionStorage.getItem(key)).at).toBe(10_000);
      await record.flush();
      record.dispose({ flushPending: false });
    } finally {
      now.mockRestore();
    }
  });

  it("bounds journal entries and total storage, and refuses body records on write and replay", async () => {
    const records = [];
    for (let index = 0; index < 5; index += 1) {
      const address = ui.uiAddress({ entityId: `journal-${index}`, view: "chat", kind: "draft" });
      const record = ui.watchUiState(address, () => {}, { debounceMs: 60_000 });
      await record.ready;
      record.schedule({ text: "a".repeat(55_000) });
      records.push(record);
    }
    window.dispatchEvent(new Event("pagehide"));
    const keys = Object.keys(sessionStorage).filter((key) => key.startsWith("build.ui.pending:"));
    const total = keys.reduce((sum, key) => sum + new TextEncoder().encode(sessionStorage.getItem(key)).length, 0);
    expect(keys.length).toBeLessThan(5);
    expect(total).toBeLessThanOrEqual(ui.UI_JOURNAL_TOTAL_MAX_BYTES);
    records.forEach((record) => record.dispose({ flushPending: false }));

    sessionStorage.clear();
    const address = ui.uiAddress({ entityId: "large", view: "chat", kind: "draft" });
    const key = `build.ui.pending:${JSON.stringify(["", address.entityId, address.kind, address.sub])}`;
    await cache.writeCached(address, { text: "kept" });
    const oversized = JSON.stringify({ at: Date.now(), source: "old", sequence: 1, value: { text: "x".repeat(ui.UI_JOURNAL_ENTRY_MAX_BYTES) } });
    sessionStorage.setItem(key, oversized);
    const restored = ui.watchUiState(address, () => {}, { debounceMs: 60_000 });
    await restored.ready;
    expect(sessionStorage.getItem(key)).toBeNull();
    expect((await cache.readCached(address)).value.text).toBe("kept");
    restored.schedule({ text: "x".repeat(ui.UI_JOURNAL_ENTRY_MAX_BYTES) });
    window.dispatchEvent(new Event("pagehide"));
    expect(sessionStorage.getItem(key)).toBeNull();
    await restored.flush();
    restored.schedule({ patch: "diff --git a/a b/a" });
    window.dispatchEvent(new Event("pagehide"));
    expect(sessionStorage.getItem(key)).toBeNull();
    await restored.flush();
    restored.dispose({ flushPending: false });
    await cache.writeCached(address, { text: "kept" });
    sessionStorage.setItem(key, JSON.stringify({ at: Date.now() + 1, source: "old", sequence: 2, value: { content_b64: "file bytes" } }));
    const refused = ui.watchUiState(address, () => {}, { debounceMs: 60_000 });
    await refused.ready;
    expect(sessionStorage.getItem(key)).toBeNull();
    expect((await cache.readCached(address)).value.text).toBe("kept");
    refused.dispose();

    for (let index = 0; index < 5; index += 1) {
      sessionStorage.setItem(`build.ui.pending:${JSON.stringify(["", `over-total-${index}`, "ui-draft", "chat:"])}`,
        JSON.stringify({ at: Date.now(), source: "other", sequence: 1, value: { text: "z".repeat(55_000) } }));
    }
    const totalAddress = ui.uiAddress({ entityId: "over-total-0", view: "chat", kind: "draft" });
    const overTotal = ui.watchUiState(totalAddress, () => {}, { debounceMs: 60_000 });
    await overTotal.ready;
    expect(Object.keys(sessionStorage).filter((entry) => entry.startsWith("build.ui.pending:"))).toHaveLength(0);
    overTotal.dispose();
  });

  it("keeps running when session storage refuses the page-exit journal", async () => {
    const address = ui.uiAddress({ entityId: "quota", view: "chat", kind: "draft" });
    const record = ui.watchUiState(address, () => {}, { debounceMs: 60_000 });
    await record.ready;
    record.schedule({ text: "still usable" });
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
    expect(() => window.dispatchEvent(new Event("pagehide"))).not.toThrow();
    setItem.mockRestore();
    await vi.waitFor(async () => expect((await cache.readCached(address))?.value.text).toBe("still usable"));
    record.dispose();
  });

  it("opens a filter menu from cache and repaints an external close", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const address = ui.uiAddress({ deviceId: "dev-1", entityId: "project-1", view: "tasks", kind: "menu", sub: "labels" });
    await cache.writeCached(address, { open: true, query: "bug" });
    const menu = menus.mountFilterMenu(host, { name: "labels", label: "Labels", onChange: () => {}, cacheAddress: address });
    menu.update([{ value: "bug", label: "Bug" }], []);
    await vi.waitFor(() => expect(menu.element.querySelector(".fmenu-pop").hidden).toBe(false));
    expect(menu.element.querySelector(".fmenu-search").value).toBe("bug");
    await cache.writeCached(address, { open: false, query: "bug" });
    await vi.waitFor(() => expect(menu.element.querySelector(".fmenu-pop").hidden).toBe(true));
    menu.dispose();
  });

  it("restores a thread run fold and repaints an external fold write", async () => {
    const address = ui.uiAddress({ deviceId: "dev-1", entityId: "run-1", view: "thread", kind: "fold", sub: "agent-1" });
    await cache.writeCached(address, { openKeys: ["42"] });
    const changed = vi.fn();
    const activity = runs.createActivityRuns({ deviceId: "dev-1", entityId: "run-1", agentId: "agent-1", call: vi.fn(), onChange: changed });
    await vi.waitFor(() => expect(activity.isOpen(42)).toBe(true));
    await cache.writeCached(address, { openKeys: [] });
    await vi.waitFor(() => expect(activity.isOpen(42)).toBe(false));
    expect(changed).toHaveBeenCalledTimes(2);
    activity.dispose();
  });
});
