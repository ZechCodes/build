// Capture the README Tasks board screenshot (docs/images/task-board.png, #301):
// the production Tasks pane in board view with sample tasks, 2x scale.
// A tool, not a test: vitest only collects test/browser/*.test.js.
// Run from spa/: npm run capture:board -- [output.png] [dark|light]
// (or node test/browser/captureBoard.mjs [output.png] [dark|light]).
// Defaults overwrite docs/images/task-board.png in the dark theme.
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const output = process.argv[2] || fileURLToPath(new URL("../../../docs/images/task-board.png", import.meta.url));
const theme = process.argv[3] || "dark";
const hello = JSON.parse(await readFile(fileURLToPath(new URL("../../../fixtures/api/v1/session.hello.json", import.meta.url)), "utf8")).result;
const projectKey = "dev-1|proj-1";
const agent = (id, workspace, name) => ({ [id]: { agent_id: id, name, ordinal: 1, workspace_name: workspace, available: true } });
const rows = [
  [214, "Offline banner for the files pane", "backlog", "low", null],
  [213, "Keyboard moves between board columns", "backlog", "none", null],
  [212, "Rate-limit retries in the sync client", "ready", "high", null],
  [211, "Remember expanded folders in the file tree", "ready", "medium", { kind: "user" }],
  [210, "Stage a single hunk from the diff", "in_progress", "urgent", { kind: "agent", agent_id: "agent-a" }, agent("agent-a", "stage-hunk", "Hunk stager")],
  [209, "Resume uploads after a dropped connection", "in_progress", "high", { kind: "agent", agent_id: "agent-b" }, agent("agent-b", "resume-uploads", "Upload fixer")],
  [208, "Search across every project's tasks", "in_review", "medium", { kind: "agent", agent_id: "agent-c" }, agent("agent-c", "task-search", "Search builder")],
  [207, "Dark theme contrast for code comments", "done", "low", null],
].map(([number, title, status, priority, assignee, identities = {}]) => ({
  id: `task-${number}`, number, title, status, priority, state: "open", assignee, identities,
  labels: [], links: { commits: [] },
}));
const feed = {
  projects: [{ projectKey, name: "demo-app" }],
  workspaces: ["stage-hunk", "resume-uploads", "task-search"].map((name, i) => ({ projectKey, entity_id: `run-${i}`, workspace_id: `ws-${i}`, name })),
  items: [{ projectKey, entity_id: "run-0", agents: [
    { id: "agent-a", name: "Hunk stager", working: true },
    { id: "agent-b", name: "Upload fixer", working: true },
    { id: "agent-c", name: "Search builder", working: false },
  ] }],
};
await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, '<main id="tasks"></main>', {
    basePath, styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block} main#tasks{margin:24px 28px;width:1376px;max-width:none}`,
  });
  await page.evaluate((wanted) => { document.documentElement.dataset.theme = wanted; }, theme);
  await loadBrowserModules(page, { changes: "src/core/changeEvents.js", tasksPane: "src/core/trackerTasksPane.js" }, basePath);
  await page.evaluate(async ({ hello, rows, feed, projectKey }) => {
    const { changes, tasksPane } = window.__layoutModules;
    const call = async (method) => {
      if (method === "session.hello") return hello;
      if (method === "tasks.list") return { tasks: rows };
      if (method === "tasks.columns") return { columns: [] };
      return {};
    };
    await changes.greetBridge(call, { deviceId: "dev-1" });
    tasksPane.mountTasksPane(document.querySelector("#tasks"), {
      deviceId: "dev-1", projectId: "proj-1", projectName: "demo-app", projectKey,
      defaultView: "board", feed: () => feed, callRpc: call,
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
  }, { hello, rows, feed, projectKey });
  await page.waitForFunction(() => document.querySelectorAll("#tasks [data-task]").length >= 8);
  await page.waitForTimeout(300);
  await page.mouse.move(0, 0);
  // Widen the pane by any horizontal overflow so every column is in frame.
  const need = await page.evaluate(() => { const b = document.querySelector(".task-board"); return b.scrollWidth - b.clientWidth; });
  await page.addStyleTag({ content: `main#tasks{width:${1376 + need}px}` });
  await page.waitForTimeout(200);
  const box = await page.locator("#tasks").boundingBox();
  const tallest = await page.evaluate(() => Math.max(...[...document.querySelectorAll("section.task-column")].map((c) => c.getBoundingClientRect().bottom)));
  await page.screenshot({ path: output, clip: { x: box.x - 24, y: box.y - 20, width: box.width + 48, height: tallest + 24 - (box.y - 20) } });
}, { width: 1640, height: 700, deviceScaleFactor: 2, plugins: [deviceShim] });
