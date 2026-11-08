// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { App, go, initRouter, unmountView } from "../src/app.js";
import { openProjectSettings } from "../src/sheets/projectSettings.js";
import { openNewRepo } from "../src/sheets/newRepo.js";
import { requestSheetDismiss } from "../src/core/sheetDismiss.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { projectSettingsAddress } from "../src/core/settingsRecords.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { environmentSupportAddress } from "../src/core/environmentSupport.js";

// Keep the real app router and page mounting; only the destination's content
// and the surrounding shell are stubs in this lifecycle integration.
vi.mock("../src/views/inbox.js", () => ({ renderInbox: () => {
  document.querySelector("#root").innerHTML = "<p>Inbox destination</p>";
} }));
vi.mock("../src/core/shell.js", async original => ({ ...await original(), standShell: vi.fn() }));
vi.mock("../src/core/inboxShell.js", async original => ({ ...await original(), inboxRouteChanged: vi.fn() }));
vi.mock("../src/core/toolbar.js", async original => ({ ...await original(), toolbarRouteChanged: vi.fn() }));
vi.mock("../src/core/cacheSync.js", async original => ({ ...await original(), routeChanged: vi.fn() }));

const readers = vi.hoisted(() => new Set());
vi.mock("../src/core/localCache.js", async original => {
  const cache = await original();
  return {
    ...cache,
    subscribeCache: (address, callback) => {
      const stop = cache.subscribeCache(address, callback);
      if (address.deviceId !== "lifetime-device") return stop;
      const reader = { address };
      readers.add(reader);
      return () => { readers.delete(reader); stop(); };
    },
  };
});

const deviceId = "lifetime-device";
const project = {
  project_id: "lifetime-project", name: "original", environment_id: "env-old",
  sources: [{ id: "folder", name: "folder", path: "/projects/folder", is_git: true }], added_sources: [],
};
const address = projectSettingsAddress(deviceId, project.project_id);
const escape = event => { if (event.key === "Escape") requestSheetDismiss(); };

beforeAll(() => {
  history.replaceState(null, "", "#/inbox");
  initRouter();
});

beforeEach(async () => {
  history.replaceState(null, "", "#/inbox");
  App.route = { name: "inbox" };
  App.gated = true;
  App.routeLeaveGuard = null;
  document.body.innerHTML = '<main id="root">Original page</main><div id="scrim"><div id="sheet"></div></div>';
  await wipeCache();
  await wipeUiRecords();
  expect(readers.size).toBe(0);
  await writeCached(address, project);
  await writeCached(environmentSupportAddress(deviceId), { addedSources: true });
  document.querySelector("#scrim").onclick = event => {
    if (event.target === document.querySelector("#scrim")) requestSheetDismiss();
  };
  document.addEventListener("keydown", escape);
});
afterEach(() => {
  unmountView();
  document.removeEventListener("keydown", escape);
  document.body.innerHTML = "";
});

async function openBrowser() {
  const callRpc = vi.fn(async method => {
    if (method === "settings.get") return { projects_dir: "/projects" };
    if (method === "fs.list") return { path: "/projects", entries: [], parent: "/" };
    return new Promise(() => {});
  });
  openProjectSettings(project.project_id, { deviceId, callRpc });
  await vi.waitFor(() => expect(document.querySelector("#psaddfolder")).toBeTruthy());
  document.querySelector("#psaddfolder").click();
  await vi.waitFor(() => expect(document.querySelector("#choosecur")).toBeTruthy());
  expect(readers.size).toBeGreaterThan(0);
}

async function pushOlderMetadata() {
  const newer = { ...project, name: "newer name", environment_id: "env-new", environment_name: "New environment" };
  await writeCached(address, newer);
  await writeCached({ deviceId, entityId: "", kind: "projects" }, [{
    ...project, sources: [{ ...project.sources[0], sync: { last_attempt_ms: 1234, ok: true } }],
  }]);
  // Drain queued record reads and writes before inspecting the actual cache.
  await vi.waitFor(() => expect(readers.size).toBe(0));
  expect((await readCached(address)).value).toEqual(newer);
}

describe("project settings opening lifetime", () => {
  it.each(["scrim", "Escape", "Close", "replacement"])("releases every reader on %s and ignores a later stale Sync push", async dismissal => {
    await openBrowser();
    if (dismissal === "scrim") document.querySelector("#scrim").click();
    if (dismissal === "Escape") document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    if (dismissal === "Close") {
      document.querySelector("#psbrowseback").click();
      await vi.waitFor(() => expect(document.querySelector("#pscancel")).toBeTruthy());
      document.querySelector("#pscancel").click();
    }
    if (dismissal === "replacement") document.querySelector("#sheet").innerHTML = "<div>Another sheet</div>";
    await pushOlderMetadata();
  });
});

describe("project settings and committed navigation", () => {
  it.each(["go", "Back"])("dismisses the browser and readers after real %s navigation", async navigation => {
    history.pushState(null, "", "#/device/lifetime-device/project/lifetime-project");
    App.route = { name: "project", deviceId, projectId: project.project_id };
    App.gated = false;
    await openBrowser();
    if (navigation === "go") expect(go({ name: "inbox" })).toBe(true);
    else history.back();
    await vi.waitFor(() => expect(document.querySelector("#root").textContent).toBe("Inbox destination"));
    expect(App.route.name).toBe("inbox");
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(false);
    await pushOlderMetadata();
  });

  it("does not dismiss an Add project replacement when navigation commits in the same turn", async () => {
    history.pushState(null, "", "#/device/lifetime-device/project/lifetime-project");
    App.route = { name: "project", deviceId, projectId: project.project_id };
    App.gated = false;
    await openBrowser();
    openNewRepo(vi.fn(), { deviceId, deviceName: "Desktop", callRpc: () => new Promise(() => {}) });
    expect(go({ name: "inbox" })).toBe(true);
    await vi.waitFor(() => expect(document.querySelector("#root").textContent).toBe("Inbox destination"));
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
    expect(document.querySelector("#nrform")).toBeTruthy();
    document.querySelector("#nrcancel").click();
    await vi.waitFor(() => expect(readers.size).toBe(0));
  });

  it("keeps the browser and its readers when the route guard refuses navigation", async () => {
    await openBrowser();
    App.routeLeaveGuard = vi.fn(async () => false);
    expect(await go({ name: "account" })).toBe(false);
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
    expect(document.querySelector("#choosecur")).toBeTruthy();
    expect(readers.size).toBeGreaterThan(0);
    document.querySelector("#scrim").click();
    await pushOlderMetadata();
  });
});
