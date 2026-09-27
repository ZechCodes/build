// Review images for #195: the mounted dashboard in both phone themes.
// Run from spa/: node test/browser/captureActiveDashboard.mjs [output directory]
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const output = process.argv[2] || "/tmp/task-195-screenshots";
await mkdir(output, { recursive: true });

const projectKey = "dev-1|proj-1";
const tasks = [
  [195, "Dashboard: group active tasks", "in_progress", "high", { kind: "agent", agent_id: "agent-busy" }],
  [194, "Check keyboard navigation", "in_review", "medium", { kind: "agent", agent_id: "agent-idle" }],
  [193, "Review color tokens", "ready", "high", { kind: "user" }],
  [192, "Write the onboarding guide", "ready", "urgent", null],
  [191, "Plan offline support", "backlog", "low", null],
].map(([number, title, status, priority, assignee]) => ({
  id: `task-${number}`, number, title, status, priority, assignee, state: "open", labels: [],
  links: { commits: [] },
}));
const feed = {
  projects: [{ projectKey, name: "Build" }],
  workspaces: [{ projectKey, entity_id: "run-1", workspace_id: "ws-1", name: "Dashboard" }],
  items: [{ projectKey, entity_id: "run-1", agents: [
    { id: "agent-busy", name: "Implementer", working: true },
    { id: "agent-idle", name: "Reviewer", working: false },
  ] }],
};

for (const theme of ["dark", "light"]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<main id="tasks"></main>', {
      basePath,
      styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block} main{margin:16px auto;max-width:760px}`,
    });
    await page.evaluate((wanted) => { document.documentElement.dataset.theme = wanted; }, theme);
    await loadBrowserModules(page, {
      changes: "src/core/changeEvents.js", tasksPane: "src/core/trackerTasksPane.js",
    }, basePath);
    await page.evaluate(async ({ projectKey, tasks, feed }) => {
      const { changes, tasksPane } = window.__layoutModules;
      const hello = { capabilities: [] };
      const call = async (method) => {
        if (method === "session.hello") return hello;
        if (method === "tasks.list") return { tasks };
        if (method === "tasks.columns") return { columns: [] };
        return {};
      };
      await changes.greetBridge(call, { deviceId: "dev-1" });
      tasksPane.mountTasksPane(document.querySelector("#tasks"), {
        deviceId: "dev-1", projectId: "proj-1", projectName: "Build", projectKey,
        defaultView: "dashboard", feed: () => feed, callRpc: call,
        catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
      });
    }, { projectKey, tasks, feed });

    for (const [tab, count, groups] of [["active", "3", ["Working", "Assigned"]], ["backlog", "2", []]]) {
      await page.locator(`[data-dashboard-tab="${tab}"]`).click();
      await page.waitForFunction(({ tab, count }) =>
        document.querySelector(`[data-dashboard-section="${tab}"]`)?.querySelectorAll("[data-task]").length === Number(count),
      { tab, count });
      const actualGroups = await page.locator(".task-dashboard-group-title").allTextContents();
      if (JSON.stringify(actualGroups) !== JSON.stringify(groups)) {
        throw new Error(`${tab}: expected ${groups}, got ${actualGroups}`);
      }
      await page.mouse.move(0, 0);
      await page.screenshot({ path: join(output, `dashboard-${tab}-phone-${theme}.png`) });
    }
  }, { width: 390, height: 844, plugins: [deviceShim] });
}
