// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openProjectSettings } from "../src/sheets/projectSettings.js";
import { requestSheetDismiss } from "../src/core/sheetDismiss.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { projectSettingsAddress } from "../src/core/settingsRecords.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { environmentSupportAddress } from "../src/core/environmentSupport.js";

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

beforeEach(async () => {
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
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
  it.each(["scrim", "Escape", "Close", "navigation", "replacement"])("releases every reader on %s and ignores a later stale Sync push", async dismissal => {
    await openBrowser();
    if (dismissal === "scrim") document.querySelector("#scrim").click();
    if (dismissal === "Escape") document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    if (dismissal === "Close") {
      document.querySelector("#psbrowseback").click();
      await vi.waitFor(() => expect(document.querySelector("#pscancel")).toBeTruthy());
      document.querySelector("#pscancel").click();
    }
    if (dismissal === "navigation") document.querySelector("#scrim").classList.remove("show");
    if (dismissal === "replacement") document.querySelector("#sheet").innerHTML = "<div>Another sheet</div>";
    await pushOlderMetadata();
  });
});
