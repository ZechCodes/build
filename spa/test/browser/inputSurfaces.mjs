import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";
import { mountSelectSurface } from "./selectSurfaces.mjs";
import { settled } from "./chatMenuSeed.mjs";

// Every visible text-entry renderer, including variants opened below. The
// onboarding gate is source-guarded: its private boot renderer cannot be mounted
// without starting account/transport orchestration. Its pairing field has the
// same code traits as the production Add device sheet captured here.
export const INPUT_SOURCES = {
  "commentPop.js": "comment popover",
  "core/agentRail.js": "new conversation",
  "core/agentRolesPanel.js": "device settings",
  "core/captureDecision.js": "task controls",
  "core/changesComposer.js": "changes composer",
  "core/compose.js": "compose",
  "core/composeView.js": "compose",
  "core/composer.js": "new conversation / task composer",
  "core/createWork.js": "task controls",
  "core/fileEditor.js": "file editor",
  "core/filterMenuControl.js": "filter menu / task composer",
  "core/inbox.js": "reroute",
  "core/taskComposer.js": "task composer",
  "core/taskReviewActions.js": "task review",
  "core/taskReviewControls.js": "task review",
  "core/taskReviewFeedback.js": "task review",
  "core/taskReviewFiles.js": "review file",
  "core/toolbar.js": "toolbar projects / toolbar workspaces",
  "core/trackerAssigneeControl.js": "task controls / assignee note",
  "core/trackerAssigneePicker.js": "assignee note",
  "core/trackerTaskRender.js": "task controls / task comment",
  "core/workspaceLifecycleSetting.js": "device settings",
  "core/workspaceRefPicker.js": "workspace refs",
  "sheets/addDevice.js": "pairing",
  "sheets/browser.js": "directory browser",
  "sheets/newRepo.js": "new project remote",
  "sheets/projectSettings.js": "project settings / project remote",
  "sheets/projectSources.js": "project settings",
  "sheets/workspaceSettings.js": "workspace folder / workspace remote",
  "views/deviceSettings.js": "device label",
  "views/gate.js": "source guard only: private onboarding pairing renderer",
};

// Nonvisual textareas are deliberately included in the source inventory audit,
// but never counted as visible form fields in browser geometry assertions.
export const INPUT_NONVISUAL_SOURCES = {
  "core/composer.js": "aria-hidden, offscreen readonly textarea mirrors the live composer for autoGrow",
  "core/connectionDiagnosticsPanel.js": "temporary transparent readonly textarea used only for clipboard fallback",
};

// Mixed modules remain in INPUT_SOURCES; these controls keep native semantics
// and are excluded from the text-entry CSS selector and browser paint checks.
export const INPUT_TOGGLE_SOURCES = {
  "core/agentRolesPanel.js": "role checkboxes",
  "core/changesReview.js": "Only changes since my review checkbox",
  "core/diffRender.js": "file selection checkboxes",
  "core/markdownBlocks.js": "markdown task checkboxes",
  "core/taskReviewActions.js": "merge and push checkboxes",
  "core/watchSetting.js": "watch tasks checkbox",
  "core/workspaceLifecycleSetting.js": "prune build output checkbox",
  "sheets/browser.js": "Show hidden checkbox",
  "sheets/sourceSyncControls.js": "sync base checkbox",
};
export const INPUT_RANGE_SOURCES = { "core/menuSlider.js": "native range slider; task #366 owns its styling" };
export const INPUT_FILE_SOURCES = {
  "core/composer.js": "hidden attachment picker",
  "core/taskComposer.js": "hidden task attachment picker",
  "core/trackerTaskRender.js": "hidden task comment attachment picker",
};
// No production renderer currently creates a radio input.
export const INPUT_RADIO_SOURCES = {};

export const INPUT_SURFACES = [
  { name: "device settings", selectSurface: "device settings", selectors: ['input[name="model"]', "#workspaceidlehours"] },
  { name: "task controls", selectSurface: "task controls", selectors: ["#task-labels", "#assign-workspace", "#capture-branch", "#capture-answer", "#create-work-input"] },
  { name: "compose", selectSurface: "compose", selectors: ["#compose-text", "#compose-branch"] },
  { name: "workspace folder", selectSurface: "workspace settings", selectors: ["#wslabel", "#wsdirpath", "#wsdirlabel"] },
  { name: "workspace remote", selectSurface: "workspace settings", selectors: ["#wslabel", "#wsdirremote", "#wsdirlabel"] },
  { name: "project settings", selectSurface: "project settings", selectors: ["#psproject", '[data-field="name"]', '[data-field="path"]', '[data-field="base_branch"]', '[data-field="remote"]'] },
  { name: "project remote", selectSurface: "project settings", selectors: ["#psremoteurl", "#pssourcelabel"] },
  { name: "new project remote", selectSurface: "new project", selectors: ["#nrproject", "[data-source-value]", "[data-source-name]", "[data-source-branch]"] },
  { name: "task review", selectSurface: "task review", selectors: ["[data-review-base]", "[data-review-push-branch]", "[data-review-description]", "#task-review-feedback-body"] },
  { name: "new conversation", selectSurface: "new conversation", selectors: ["[data-new-agent-name]", ".rail-composer textarea"] },
  { name: "pairing", selectors: ["#paircode"] },
  { name: "directory browser", selectors: ["#bdirnew"] },
  { name: "device label", selectors: ["#device-label"] },
  { name: "filter menu", selectors: [".fmenu-search"] },
  { name: "workspace refs", selectors: [".workspace-refsearch"] },
  { name: "toolbar projects", selectors: ['.tb-filter[aria-label="Filter projects"]'] },
  { name: "toolbar workspaces", selectors: ['.tb-filter[aria-label="Filter workspaces"]'] },
  { name: "reroute", selectors: ["[data-reroute-branch]"] },
  { name: "task composer", selectors: [".task-compose-title", ".task-compose-body textarea", ".fmenu-search"] },
  { name: "task comment", selectors: ["#task-comment"] },
  { name: "assignee note", selectors: [".modal-create textarea"] },
  { name: "comment popover", selectors: [".cp-input"] },
  { name: "changes composer", selectors: [".csinput"] },
  { name: "file editor", selectors: [".file-editor"] },
  { name: "review file", selectors: ["[data-review-line-input]"] },
];

const modules = {
  app: "src/app.js", cache: "src/core/localCache.js", records: "src/core/settingsRecords.js",
  pairing: "src/sheets/addDevice.js", browser: "src/sheets/browser.js", device: "src/views/deviceSettings.js",
  filter: "src/core/filterMenuControl.js", refs: "src/core/workspaceRefPicker.js", toolbar: "src/core/toolbar.js",
  feed: "src/core/taskFeed.js", inbox: "src/core/inbox.js", taskComposer: "src/core/taskComposer.js",
  task: "src/core/trackerTaskRender.js", assignee: "src/core/trackerAssigneePicker.js", pop: "src/commentPop.js",
  changes: "src/core/changesComposer.js", editor: "src/core/fileEditor.js", reviewFiles: "src/core/taskReviewFiles.js",
};

// All data stays in the browser's disposable cache. No fixture opens a bridge
// connection: the device label page uses its existing offline render path.
async function seedInputSurface({ name, theme }) {
  document.documentElement.dataset.theme = theme;
  const m = window.__layoutModules;
  const root = document.querySelector("#root");
  const device = { id: "input-device", name: "Laptop", status: "offline" };
  const project = { id: "input-project", name: "Build", deviceId: device.id, sources: [] };
  const workspace = { id: "input-workspace", name: "Input styling", project_id: project.id, deviceId: device.id };
  const scope = { deviceId: device.id, projectId: project.id, taskId: "input-task" };
  const task = { id: scope.taskId, number: 368, title: "Style text inputs", state: "open", status: "backlog", priority: "medium", labels: [] };
  const options = [{ id: "none", label: "Nobody" }, { id: "workspace", label: "New workspace", form: "workspace" }];
  const disposers = [];
  const dispose = (mounted) => disposers.push(() => mounted?.dispose?.());
  window.__inputDispose = () => disposers.forEach((stop) => stop());
  m.app.App.devices = [device];
  m.app.App.selectedDeviceId = device.id;
  m.app.App.deviceFilter = "all";
  const listing = { path: "/code", parent: "/", is_git: false, entries: [{ name: "Build", path: "/code/Build", is_git: true }] };
  const refs = { refs: [{ name: "main", full_ref: "refs/heads/main", kind: "branch", current: true }] };
  const callRpc = async (method) => ({ "fs.list": listing, "git.refs": refs })[method] || {};
  await m.cache.writeCached({ deviceId: "", entityId: "", kind: "devices" }, [device]);
  await m.cache.writeCached({ deviceId: device.id, entityId: "", kind: "projects" }, [project]);
  await m.cache.writeCached({ deviceId: device.id, entityId: "", kind: "workspaces" }, [workspace]);

  const mounts = {
    pairing() { disposers.push(m.pairing.openAddDevice(() => {})); },
    "directory browser": async () => {
      await m.cache.writeCached(m.browser.browserListingAddress(device.id, "/code"), listing);
      await m.browser.openBrowser({ title: "Choose a folder", deviceId: device.id, startPath: "/code", allowCreateDirectory: true, callRpc, onChoose: () => {} });
    },
    "device label": async () => {
      await m.cache.writeCached(m.records.deviceSettingsAddress(device.id), { projects_dir: "/code" });
      await m.device.renderDeviceSettings({ root, deviceId: device.id, registerDispose: (stop) => disposers.push(stop) });
    },
    "filter menu"() {
      const menu = m.filter.mountFilterMenu(root, { name: "labels", label: "Labels", onChange: () => {} });
      menu.update([{ id: "ui", label: "UI" }], []);
      dispose(menu);
    },
    "workspace refs": async () => {
      const address = { deviceId: device.id, entityId: "input-refs", kind: "refs" };
      await m.cache.writeCached(address, refs);
      dispose(m.refs.mountWorkspaceRefPicker(root, { scope: { entity_id: "input-refs" }, callRpc, cacheScope: { deviceId: device.id, address: () => address } }));
    },
    "toolbar projects": async () => {
      m.app.App.route = { name: "project", projectId: project.id, deviceId: device.id };
      await m.feed.startFeed();
      await m.toolbar.initToolbar();
      disposers.push(() => { m.toolbar.stopToolbar(); m.feed.stopFeed(); });
    },
    "toolbar workspaces": async () => {
      m.app.App.route = { name: "workspace", projectId: project.id, workspaceId: workspace.id, deviceId: device.id };
      await m.feed.startFeed();
      await m.toolbar.initToolbar();
      disposers.push(() => { m.toolbar.stopToolbar(); m.feed.stopFeed(); });
    },
    reroute() {
      const entry = { key: "capture:input", captureId: "input", captureState: "routed", name: "Restyle inputs", text: "Restyle inputs", state: "open" };
      root.innerHTML = m.inbox.captureRowHtml(entry, { rerouteKey: entry.key, projects: [project], rerouteBranchProject: project.id, rerouteBranches: ["main"] });
    },
    "task composer"() {
      const composer = m.taskComposer.openTaskComposer(root, { ...scope, projectName: project.name, columns: [{ id: "backlog", name: "Backlog" }], labels: ["ui"], options, callRpc });
      disposers.push(() => composer.close());
    },
    "task comment"() { root.innerHTML = m.task.composerHtml("", false); },
    "assignee note"() { m.assignee.openAssigneePicker({ task, options, note: "Review the field styling", callRpc }); },
    "comment popover"() {
      m.pop.openCommentComposer({ left: 16, bottom: 80 }, () => {}, root);
      disposers.push(() => m.pop.hideCommentPop(root));
    },
    "changes composer"() {
      const composer = m.changes.mountChangesComposer(root, { offers: () => ({ commentable: true, uncommitted: true }), run: async () => {}, readDraft: () => "", writeDraft: () => {} });
      dispose(composer);
    },
    "file editor"() { dispose(m.editor.mountFileEditor(root, { value: "// The editor preserves its own layout.\nconst styled = true;\n" })); },
    "review file"() {
      const file = { path: "example.js", size: 20, mime: "text/javascript", content_b64: btoa("const styled = true;\n") };
      const readReview = async (_method, params) => params.mode === "tree" ? { entries: [{ name: file.path, path: file.path, kind: "file" }] } : file;
      dispose(m.reviewFiles.mountTaskReviewFiles(root, { ...scope, snapshot: { id: "input-snapshot" }, directory: { id: "input-directory", name: "Build", is_git: true }, path: file.path, callRpc: readReview, onComment: () => {} }));
    },
  };
  await mounts[name]();
}

async function mountExtraSurface(page, basePath, name, theme) {
  await mountLayout(page, '<div id="shell"><div id="view"><div id="toolbar"></div><main id="root" class="surface"></main></div></div><div id="scrim" class="scrim"><div id="sheet" class="sheet"></div></div>', {
    basePath, styles: '#root{padding:16px;overflow:auto;min-width:0} #view{min-width:0}',
  });
  await loadBrowserModules(page, { app: modules.app }, basePath);
  await page.evaluate(() => { delete window.__layoutModules; delete window.__layoutModuleError; });
  await loadBrowserModules(page, modules, basePath);
  await page.evaluate(seedInputSurface, { name, theme });
}

async function openInputVariant(page, name) {
  const variants = {
    "workspace folder": () => page.selectOption("#wsdiradd", "path"),
    "workspace remote": () => page.selectOption("#wsdiradd", "remote"),
    "project remote": () => page.locator("#psaddremote").click(),
    "new project remote": () => page.locator("#nraddremote").click(),
    "filter menu": () => page.locator(".fmenu-press").click(),
    "workspace refs": () => page.locator("[data-refpicker-toggle]").click(),
    "toolbar projects": () => page.locator('[data-select="project"]').click(),
    "toolbar workspaces": () => page.locator('[data-select="workspace"]').click(),
    "task composer": () => page.locator(".fmenu-press").first().click(),
  };
  await variants[name]?.();
}

export async function mountInputSurface(page, basePath, name, theme) {
  const surface = INPUT_SURFACES.find((entry) => entry.name === name);
  if (!surface) throw new Error(`Unknown input surface: ${name}`);
  page.setDefaultTimeout(5000);
  if (surface.selectSurface) {
    await mountSelectSurface(page, basePath, surface.selectSurface, theme);
    await page.evaluate(() => { window.__inputDispose = () => window.__selectDispose?.(); });
  } else await mountExtraSurface(page, basePath, name, theme);
  await openInputVariant(page, name);
  await openInputDisclosures(page);
  for (const selector of surface.selectors) await page.waitForSelector(selector, { state: "attached" });
  await settled(page);
}

export async function openInputDisclosures(page) {
  await page.evaluate(() => document.querySelectorAll("details:has(input, textarea)").forEach((details) => { details.open = true; }));
  await settled(page);
}
