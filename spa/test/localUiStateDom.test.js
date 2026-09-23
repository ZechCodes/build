// @vitest-environment jsdom
// UI records use the real IndexedDB cache and its address announcements.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, ui, focus, menus, runs;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
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
    expect(document.activeElement).toBe(root.querySelector("#second"));
    dispose();
  });

  it("opens a filter menu from cache and repaints an external close", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const address = ui.uiAddress({ deviceId: "dev-1", entityId: "project-1", view: "issues", kind: "menu", sub: "labels" });
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
