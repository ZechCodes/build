// @vitest-environment jsdom
// The rail's presentation choices use the real IndexedDB cache and its
// announcement/readback path, including a remount with no user action.
import { beforeEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const setInboxView = vi.fn();
vi.mock("../src/app.js", () => ({ App: { route: { name: "inbox" } }, go: vi.fn() }));
vi.mock("../src/core/inboxView.js", () => ({
  mountInboxList: vi.fn(),
  inboxListRouteChanged: vi.fn(),
  openNewProject: vi.fn(),
  setInboxView: (...args) => setInboxView(...args),
}));

let cache, ui, shell;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  setInboxView.mockClear();
  vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: false, addEventListener: () => {} })));
  cache = await import("../src/core/localCache.js");
  ui = await import("../src/core/localUiState.js");
  shell = await import("../src/core/inboxShell.js");
});

it("mounts a cached fold and face without a click", async () => {
  await cache.writeCached(ui.uiAddress({ view: "inbox", kind: "fold", sub: "rail" }), { collapsed: true });
  await cache.writeCached(ui.uiAddress({ view: "inbox", kind: "filter", sub: "face" }), { view: "projects" });
  await shell.initInboxRail();
  expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
  expect(document.querySelector('[data-inbox-view="projects"]').getAttribute("aria-pressed")).toBe("true");
  expect(setInboxView).toHaveBeenLastCalledWith("projects");
});

it("repaints a mounted rail after an external cache write", async () => {
  await shell.initInboxRail();
  const foldAddress = ui.uiAddress({ view: "inbox", kind: "fold", sub: "rail" });
  const faceAddress = ui.uiAddress({ view: "inbox", kind: "filter", sub: "face" });
  await cache.writeCached(foldAddress, { collapsed: true });
  await vi.waitFor(() => expect(document.body.classList.contains("inbox-collapsed")).toBe(true));
  await cache.writeCached(faceAddress, { view: "projects" });
  await vi.waitFor(() => expect(setInboxView).toHaveBeenLastCalledWith("projects"));
  expect((await cache.readCached(foldAddress)).value.collapsed).toBe(true);
});
