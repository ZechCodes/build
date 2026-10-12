import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

export const remote = "git@github.com:org/zimmerman-web-works.git";
export const failure = "Could not clone repository: Permission denied (publickey).";

// Hold the success cache write so evidence can capture the brief Done state.
// Releasing it runs the real cache path before onDone, just as production does.
export const holdProjectCache = {
  name: "new-project-progress-cache-barrier",
  enforce: "pre",
  resolveId(source, importer) {
    if (!importer?.endsWith("/sheets/newRepo.js")) return null;
    if (source === "../core/settingsRecords.js") return "\0new-project-progress-settings";
    if (source === "../core/localUiState.js") return "\0new-project-progress-ui";
    return null;
  },
  load(id) {
    if (id === "\0new-project-progress-ui") return `export * from '/src/core/localUiState.js';
      import { watchUiState as watch } from '/src/core/localUiState.js';
      export function watchUiState(...args) {
        const record = watch(...args);
        return { ...record, async write(value) {
          if (value.sources?.length === 0) await window.__draftClearReady;
          return record.write(value);
        } };
      }`;
    if (id !== "\0new-project-progress-settings") return null;
    return `export * from '/src/core/settingsRecords.js';
      import { writeProjectSetting as write } from '/src/core/settingsRecords.js';
      export async function writeProjectSetting(...args) {
        await window.__projectCacheReady;
        return write(...args);
      }`;
  },
};

export async function mountNewProject(page, basePath, { local = false } = {}) {
  await mountLayout(page, '<div class="scrim" id="scrim"><div class="sheet" id="sheet"></div></div>', { basePath });
  await loadBrowserModules(page, {
    newRepo: "src/sheets/newRepo.js", dismiss: "src/core/sheetDismiss.js",
    ui: "src/core/localUiState.js", store: "src/core/localUiStore.js",
    cache: "src/core/localCache.js", settings: "src/core/settingsRecords.js",
  }, basePath);
  await page.evaluate(async ({ local }) => {
    const { newRepo, dismiss, ui, store } = window.__layoutModules;
    window.__projectDraftAddress = ui.uiAddress({ view: "new-project", kind: "draft" });
    if (local) await store.writeUiRecord(window.__projectDraftAddress, {
      name: "", nameAutomatic: true, selectedDeviceId: "beast",
      sources: [{ id: 1, kind: "path", path: "/home/you/code/docs", pathDeviceId: "beast", name: "docs", base_branch: "", automaticName: true }],
    });
    window.__projectCacheReady = new Promise((resolve) => { window.__releaseProjectCache = resolve; });
    window.__draftClearReady = Promise.resolve();
    window.__projectRequests = [];
    const call = (method, params) => {
      if (method !== "project.create") return Promise.resolve({});
      window.__projectRequests.push(params);
      return new Promise((resolve, reject) => { window.__resolveProject = resolve; window.__rejectProject = (message) => reject(new Error(message)); });
    };
    const sheet = newRepo.openNewRepo((project, target) => { window.__openedProject = { project, target }; }, {
      devices: [{ id: "beast", name: "The Beast" }], callRpcFor: () => call,
    });
    document.querySelector("#scrim").onclick = (event) => { if (event.target.id === "scrim") dismiss.requestSheetDismiss(); };
    document.addEventListener("keydown", (event) => { if (event.key === "Escape") dismiss.requestSheetDismiss(); });
    await sheet.whenDraftRestored;
  }, { local });
  await page.locator("#nraddremote").click();
  await page.locator("[data-source-value]").fill(remote);
  await page.locator("[data-source-branch]").last().fill("main");
  await page.locator("#nrproject").fill("Zimmerman Web Works");
}

export async function resolveProject(page) {
  await page.evaluate(() => window.__resolveProject({ project_id: "zimmerman", name: "Zimmerman Web Works" }));
}
