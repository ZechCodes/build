// @vitest-environment jsdom
// #628: real cache announcements move mounted rows and project blocks. Time
// alone also moves a row into Recent; no bridge session or polling is needed.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const DEVICE = "inbox-live-device";
const HOUR = 3600_000;
const DAY = 24 * HOUR;
const NOW = Date.parse("2026-10-16T12:00:00Z");
const WAIT = { timeout: 5000, interval: 20 };
const address = (kind) => ({ deviceId: DEVICE, entityId: "", kind });
const project = (id, name) => ({ id, project_id: id, name, deviceId: DEVICE, projectKey: `${DEVICE}/${id}` });
const workspace = (id, projectId, startMs, lastMs) => ({ id, project_id: projectId, name: id,
  deviceId: DEVICE, projectKey: `${DEVICE}/${projectId}`, workspaceKey: `${DEVICE}/${id}`,
  status: "ready", session_started_ms: startMs, last_activity_ms: lastMs });
const activeRows = () => [...document.querySelectorAll("#inbox-list .inbox-entry:not(.inbox-project-agent)")]
  .filter((row) => !row.closest(".inbox-recent"));
const activeKeys = () => activeRows().map((row) => row.dataset.key);
const blockKeys = () => [...document.querySelectorAll("#inbox-list .inbox-project")].map((block) => block.dataset.project);
const rowKey = (id) => `workspace:${DEVICE}/${id}`;
const projects = [project("alpha", "Alpha"), project("zulu", "Zulu")];
let modules;

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
  vi.setSystemTime(NOW);
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = bodyHtml;
  const { App } = await import("../src/app.js");
  Object.assign(App, { route: { name: "inbox" }, devices: [{ id: DEVICE, name: "Laptop", status: "online" }],
    selectedDeviceId: DEVICE, deviceFilter: null });
  modules = {
    cache: await import("../src/core/localCache.js"),
    taskFeed: await import("../src/core/taskFeed.js"),
    inboxView: await import("../src/core/inboxView.js"),
    contexts: await import("../src/core/deviceContexts.js"),
  };
  await modules.cache.writeCached(address("projects"), projects);
});

afterEach(() => {
  modules?.inboxView.unmountInboxList();
  modules?.taskFeed.stopFeed();
  modules?.contexts.resetDeviceContexts();
  vi.useRealTimers();
});

async function boot(view, rows) {
  await modules.cache.writeCached(address("workspaces"), rows);
  modules.inboxView.setInboxView(view);
  modules.inboxView.mountInboxList();
  await modules.taskFeed.startFeed();
  await vi.waitFor(() => expect(activeRows()).toHaveLength(rows.length), WAIT);
}

it.each(["inbox", "projects"])("moves rows and blocks after a cached activity push in %s", async (view) => {
  const longer = workspace("longer", "zulu", NOW - 6 * HOUR, NOW - HOUR);
  const shorter = workspace("shorter", "alpha", NOW - 2 * HOUR, NOW - HOUR);
  await boot(view, [shorter, longer]);
  expect(activeKeys()).toEqual([rowKey("longer"), rowKey("shorter")]);
  if (view === "projects") expect(blockKeys()).toEqual([`${DEVICE}/zulu`, `${DEVICE}/alpha`]);
  const kept = activeRows().find((row) => row.dataset.key === rowKey("longer"));

  // A new streak begins after a long gap. The existing row must move without
  // any navigation, remount, reconnect, or second cache write.
  await modules.cache.writeCached(address("workspaces"), [shorter,
    { ...longer, session_started_ms: NOW, last_activity_ms: NOW }]);
  await vi.waitFor(() => expect(activeKeys()).toEqual([rowKey("shorter"), rowKey("longer")]), WAIT);
  expect(activeRows().find((row) => row.dataset.key === rowKey("longer"))).toBe(kept);
  if (view === "projects") expect(blockKeys()).toEqual([`${DEVICE}/alpha`, `${DEVICE}/zulu`]);
});

it.each(["inbox", "projects"])("folds at the exact 24h cutoff without another push in %s", async (view) => {
  const deadline = NOW + 1000;
  await boot(view, [workspace("aging", "zulu", deadline - DAY - HOUR, deadline - DAY)]);
  await vi.advanceTimersByTimeAsync(deadline - Date.now() - 1);
  expect(activeKeys()).toEqual([rowKey("aging")]);
  await vi.advanceTimersByTimeAsync(1);
  expect(activeKeys()).toEqual([]);
  const recent = document.querySelector('[data-recent-toggle]');
  expect(recent).not.toBeNull();
  if (view === "projects") expect(recent.closest(".inbox-project").dataset.project).toBe(`${DEVICE}/zulu`);
});

it.each(["inbox", "projects"])("rechecks suspended time when the page becomes visible in %s", async (view) => {
  await boot(view, [workspace("suspended", "zulu", NOW - HOUR, NOW)]);
  vi.setSystemTime(NOW + DAY);
  document.dispatchEvent(new Event("visibilitychange"));
  expect(activeKeys()).toEqual([]);
});

it("reschedules from the newest cached activity and cancels the clock on unmount", async () => {
  const aging = workspace("aging", "zulu", NOW - DAY - HOUR, NOW - DAY + 1000);
  await boot("inbox", [aging]);
  await modules.cache.writeCached(address("workspaces"), [{ ...aging, name: "Renewed", last_activity_ms: NOW }]);
  await vi.waitFor(() => expect(activeRows()[0]?.textContent).toContain("Renewed"), WAIT);
  await vi.advanceTimersByTimeAsync(1000);
  expect(activeKeys()).toEqual([rowKey("aging")]);
  const pending = vi.getTimerCount();
  modules.inboxView.unmountInboxList();
  expect(vi.getTimerCount()).toBe(pending - 1);
  vi.setSystemTime(NOW + DAY);
  document.dispatchEvent(new Event("visibilitychange"));
  expect(activeKeys()).toEqual([rowKey("aging")]);
});
