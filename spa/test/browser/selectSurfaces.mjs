import reviewFixture from "../../../fixtures/api/v1/tasks.review.get.json" with { type: "json" };
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";
import { mountChatMenu, settled } from "./chatMenuSeed.mjs";

export const SELECT_SOURCES = {
  "core/agentChoice.js": "task controls / compose / sheets",
  "core/agentModes.js": "device settings",
  "core/agentRail.js": "new conversation / clear conversation",
  "core/agentRolesPanel.js": "device settings",
  "core/captureDecision.js": "task controls",
  "core/composeView.js": "compose",
  "core/createWork.js": "task controls",
  "core/defaultHarness.js": "device settings",
  "core/diffSort.js": "task controls",
  "core/harnessDefaults.js": "local settings / workspace settings",
  "core/isolation.js": "device settings / project settings",
  "core/projectAgentSetting.js": "device settings",
  "core/taskReviewActions.js": "task review",
  "core/taskReviewControls.js": "task review",
  "core/taskReviewFeedback.js": "task review",
  "core/taskReviewRender.js": "task review",
  "core/trackerAssigneeControl.js": "task controls",
  "core/trackerTaskRender.js": "task controls",
  "core/workspaceReviewEntry.js": "workspace review",
  "sheets/newRepo.js": "new project",
  "sheets/workspaceSettings.js": "workspace settings",
  "views/settings.js": "local settings",
};

export const SELECT_SURFACES = [
  { name: "local settings", selectors: ["#creationdev", "#defprovider", "[data-harness-model]", "[data-harness-effort]"] },
  { name: "device settings", selectors: ["[data-agent-mode]", "#defaultharness", "#projectagentharness", "#projectagentmodel", "#projectagenteffort", "[data-capability]", '[data-isolation="select"]'] },
  { name: "task controls", selectors: ["#task-status", "#task-priority", "#assign-assignee", "#assign-isolation", "#assign-choice-provider", "#assign-choice-model", "#assign-choice-effort", "#capture-project", "#create-work-isolation", ".diffsort-select", "select.mini"] },
  { name: "compose", selectors: ["#compose-project", "#compose-choice-provider", "#compose-choice-model", "#compose-choice-effort"] },
  { name: "workspace settings", selectors: ["#wsdiradd", "#wsdefprovider", "[data-harness-model]", "[data-harness-effort]"] },
  { name: "project settings", selectors: ['[data-isolation="select"]'] },
  { name: "new project", selectors: ["#nrdevice"] },
  { name: "workspace review", selectors: ["[data-review-task]"] },
  { name: "task review", selectors: ["[data-review-snapshot]", "[data-review-workspace]", "[data-review-opinion]", "[data-review-merge-branch]", "[data-review-remote]"] },
  { name: "new conversation", selectors: ["[data-new-agent-role]"] },
  { name: "clear conversation", selectors: ["[data-clear-detail]", "[data-clear-compact]"] },
];

const catalog = {
  default_provider: "claude_adk", providers: [
    { id: "claude_adk", label: "Claude Code", models: [{ id: "claude-opus-4", label: "Claude Opus 4", supports_effort: true }], efforts: [{ id: "low", label: "Low" }, { id: "high", label: "High" }] },
    { id: "codex", label: "Codex", models: [{ id: "gpt-6", label: "GPT-6", supports_effort: true }], efforts: [{ id: "low", label: "Low" }, { id: "high", label: "High" }] },
  ],
};
const device = { id: "select-device", name: "Laptop", status: "online", fingerprint: "fixture-key" };
const project = { id: "select-project", project_id: "select-project", name: "Build", path: "/code/Build", is_git: true, base_branch: "main", isolation_default: "worktree", isolation_effective: "worktree", isolation_available: { rift: true }, sources: [{ id: "source-web", name: "Website", path: "/code/website", is_git: true }] };
const settings = { projects_dir: "/code", default_harness: "claude_adk", project_agent: { provider: "claude_adk", model: "claude-opus-4", effort: "high" }, agent_modes: { claude: "headless", codex: "tui" }, role_models: [{ model: "gpt-6", roles: ["implementer"], capability: "scoped" }], isolation: "worktree", isolation_available: { rift: true }, watch_tasks: true, workspace_idle_minutes: 15, workspace_reclaim_build_artifacts: false };
const modules = {
  app: "src/app.js", cache: "src/core/localCache.js", records: "src/core/settingsRecords.js",
  settings: "src/views/settings.js", panels: "src/views/devicePanels.js", contexts: "src/core/deviceContexts.js",
  defaults: "src/core/harnessDefaults.js", task: "src/core/trackerTaskRender.js", assignee: "src/core/trackerAssigneeControl.js",
  capture: "src/core/captureDecision.js", create: "src/core/createWork.js", sort: "src/core/diffSort.js",
  compose: "src/core/composeView.js", workspace: "src/sheets/workspaceSettings.js", newRepo: "src/sheets/newRepo.js",
  project: "src/sheets/projectSettings.js", reviewEntry: "src/core/workspaceReviewEntry.js", tracker: "src/core/trackerCache.js",
  review: "src/core/taskReviewPage.js", reviewSupport: "src/core/taskReviewSupport.js", reviewCache: "src/core/taskReviewCache.js",
};

async function loadSurfaceModules(page, basePath) {
  // app's circular imports must settle before the individual view modules.
  await loadBrowserModules(page, { app: modules.app }, basePath);
  await page.evaluate(() => { delete window.__layoutModules; delete window.__layoutModuleError; });
  await loadBrowserModules(page, modules, basePath);
}

async function seedSelectSurface({ name, theme, catalog, device, project, settings, savedReview }) {
  document.documentElement.dataset.theme = theme;
  const m = window.__layoutModules;
  const root = document.querySelector("#root");
  m.app.App.devices = [device];
  m.app.App.selectedDeviceId = device.id;
  const disposers = [];
  window.__selectDispose = () => disposers.forEach((dispose) => dispose?.());
  window.__selectCalls = [];
  const callRpc = async (method, params = {}) => {
    window.__selectCalls.push({ method, params });
    if (method === "models.list") return catalog;
    if (method === "project.list") return { projects: [project] };
    if (method === "workspace.get") return { id: "workspace-1", project_id: project.id, directories: [] };
    if (method === "settings.set") return { ...settings, ...params, agent_modes: { ...settings.agent_modes, ...params.agent_modes } };
    if (method === "tasks.review.get") return { review: savedReview };
    if (method === "tasks.review.diff") return { stat: { files_changed: 0, insertions: 0, deletions: 0 }, files: [], files_truncated: false, diff_key: "base:head" };
    return settings;
  };
  await m.cache.writeCached(m.records.deviceModelsAddress(device.id), catalog);
  await m.cache.writeCached(m.records.deviceSettingsAddress(device.id), settings);
  await m.cache.writeCached(m.records.projectSettingsAddress(device.id, project.id), project);
  await m.cache.writeCached({ deviceId: device.id, entityId: "", kind: "projects" }, [project]);
  await m.cache.writeCached({ deviceId: "", entityId: "", kind: "devices" }, [device]);
  const scope = { deviceId: device.id, projectId: project.id, taskId: "task-1" };
  const support = { get: true, snapshot: true, diff: true, complete: true, act: true, comments: true };
  await m.reviewSupport.rememberReviewSupport(device.id, { reviews: support });
  if (name === "local settings") {
    await m.settings.renderSettings({ root, registerDispose: (dispose) => disposers.push(dispose) });
    m.defaults.mountHarnessDefaults(root, { catalog });
  }
  if (name === "device settings") {
    root.innerHTML = '<div id="device-projects-panel"></div><div id="device-bridge-panels"></div>';
    disposers.push(m.panels.standUpDevicePanels({ projectsHost: root.firstElementChild, bridgeHost: root.lastElementChild, callRpc, device }));
  }
  if (name === "task controls") {
    const task = { id: "task-1", state: "open", status: "backlog", priority: "medium", labels: [] };
    const rail = m.task.taskRailHtml(task, { ...scope, columns: [{ id: "backlog", name: "Backlog" }, { id: "review", name: "In review" }], links: [], labelsDraft: "", busy: false });
    const options = [{ id: "none", label: "Nobody" }, { id: "workspace", label: "New workspace", form: "workspace" }];
    const draft = { optionId: "workspace", name: "", isolation: "", choiceOpen: true, choice: { provider: "claude_adk", model: "claude-opus-4", effort: "high" } };
    const assignee = m.assignee.assigneeControlHtml(options, draft, { prefix: "assign", catalog });
    const capture = m.capture.captureDecisionHtml(m.capture.captureDecisionModel({ id: "capture-1", text: "Fix selects", state: "needs_input" }), { projects: [project], projectId: project.id });
    root.innerHTML = rail + `<div class="modal-create">${assignee}</div>` + capture + `<div class="modal-create">${m.create.createWorkBodyHtml({ projectId: project.id, projectName: project.name, name: "", isolation: "", busy: false })}</div>` + m.sort.diffSortHtml("latest") + '<label>Compact control<select class="mini"><option>First</option><option>Second</option></select></label>';
  }
  if (name === "compose") {
    root.innerHTML = '<div id="compose"></div>';
    m.compose.initCompose();
    m.compose.openCompose();
  }
  if (name === "workspace settings") {
    disposers.push(m.workspace.openWorkspaceSettings({ id: "workspace-1", name: "Select styling", workspaceKey: `${device.id}/workspace-1` }, { callRpc, catalog, deviceId: device.id }));
  }
  if (name === "project settings") {
    m.project.openProjectSettings(project.id, { callRpc, deviceId: device.id });
  }
  if (name === "new project") {
    disposers.push(m.newRepo.openNewRepo(() => {}, { devices: [device, { ...device, id: "desktop", name: "Desktop" }], defaultDeviceId: device.id, callRpcFor: () => callRpc }));
  }
  if (name === "workspace review") {
    await m.cache.writeCached(m.tracker.tasksAddress(device.id, project.id), { tasks: [{ id: "task-1", number: 367, title: "Style select menus" }], columns: [] });
    const entry = m.reviewEntry.mountWorkspaceReviewEntry(root, { ...scope, workspaceId: "workspace-1", callRpc });
    disposers.push(entry.dispose);
  }
  if (name === "task review") {
    savedReview.destinations = [{ snapshot_id: savedReview.snapshots[0].id, directory_id: "dir-api", source_path: "/code/Build", branches: ["main", "release"], remotes: [{ name: "origin", branches: ["main"] }, { name: "backup", branches: ["main"] }], live_head: "3".repeat(40) }];
    savedReview.actions = [];
    await m.reviewCache.writeReviewRecord(scope, savedReview, 1);
    const review = m.review.mountTaskReviewPage(root, { ...scope, callRpc, workspaces: () => [{ id: "workspace-1", name: "Select styling" }], task: () => ({ id: "task-1" }) });
    disposers.push(review.dispose);
  }
}

export async function mountSelectSurface(page, basePath, name, theme) {
  page.setDefaultTimeout(5000);
  if (name.endsWith("conversation")) {
    await mountChatMenu(page, basePath, "phone", { theme, bigCounts: false, resetCapable: true });
    if (name === "new conversation") await page.locator('[data-bubble="add"]').click();
    else {
      await page.locator(".rail-surface-menu .caret").click();
      await page.locator('[data-action="conversation:clear"]').click();
      await page.locator("[data-confirm-ok]").click();
    }
    await settled(page);
    return;
  }
  await mountLayout(page, '<div id="shell"><div id="view"><main id="root" class="surface"></main></div></div><div id="scrim" class="scrim"><div id="sheet" class="sheet"></div></div>', { basePath, styles: '#root{padding:16px;overflow:auto} #view{min-width:0}' });
  await page.route("**/api/devices", (route) => route.fulfill({ json: { devices: [device] } }));
  await page.route("**/app/downloads", (route) => route.fulfill({ status: 503, json: { detail: "Fixture downloads unavailable" } }));
  await loadSurfaceModules(page, basePath);
  await page.evaluate(seedSelectSurface, { name, theme, catalog, device, project, settings, savedReview: reviewFixture.result.review });
  if (name === "compose") {
    await page.locator("#compose-advanced").click();
    await page.locator('[data-agent-choice-toggle="compose-choice"]').click();
  }
  if (name === "workspace review") await page.locator("[data-workspace-review]").click();
  for (const selector of SELECT_SURFACES.find((surface) => surface.name === name).selectors) {
    await page.waitForSelector(selector, { state: "attached", timeout: 5000 });
  }
  await settled(page);
}

export async function openSelectDisclosures(page) {
  await page.evaluate(() => document.querySelectorAll("details:has(select)").forEach((details) => { details.open = true; }));
  await settled(page);
}
