// Capture the mounted Dashboard's flat Backlog tab. Assigned tasks now appear
// in Active (#195), so only unassigned rows remain here.
// Run from spa/: node test/browser/captureBacklogGroups.mjs [output.png] [width] [height]
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./issueIdentityHarness.mjs";

const output = process.argv[2] || "/tmp/backlog-groups.png";
const width = Number(process.argv[3]) || 1440;
const height = Number(process.argv[4]) || 900;
const fixture = (name) => readFile(fileURLToPath(new URL(`../../../fixtures/api/v1/${name}.json`, import.meta.url)), "utf8")
  .then(JSON.parse);
const hello = (await fixture("session.hello")).result;

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, '<main id="issues"></main>', {
    basePath, styles: `@import url("${basePath}src/styles/issues.css"); body{display:block} main{max-width:760px;margin:24px auto}`,
  });
  await loadBrowserModules(page, {
    changes: "src/core/changeEvents.js", issuesPane: "src/core/trackerIssuesPane.js",
  }, basePath);
  await page.evaluate(async (hello) => {
    const { changes, issuesPane } = window.__layoutModules;
    const agent = (id, workspace, name) => ({ [id]: { agent_id: id, name, ordinal: 1, workspace_name: workspace, available: true } });
    const waiting = [
      [168, "Board: keyboard moves between columns", "backlog", "none", null],
      [167, "Chat composer keeps its draft across reloads", "ready", "high",
        { kind: "agent", agent_id: "agent-a" }, agent("agent-a", "composer-draft", "Draft keeper")],
      [166, "Files explorer remembers expanded folders", "ready", "medium", null],
      [165, "Landing: pricing section copy", "backlog", "none", { kind: "user" }],
      [164, "Git pane: stage a hunk from the diff", "backlog", "urgent", null],
      [163, "Agent rail: unread dot clears on focus", "backlog", "low",
        { kind: "agent", agent_id: "agent-b" }, agent("agent-b", "agent-rail", "Unread dots")],
      [162, "Issues: bulk relabel from the list", "backlog", "none", null],
    ].map(([number, title, status, priority, assignee, identities = {}]) => ({
      id: `issue-${number}`, number, title, status, priority, state: "open", assignee, identities, labels: [],
      links: { commits: [] },
    }));
    const call = async (method) => {
      if (method === "session.hello") return hello;
      if (method === "issues.list") return { issues: waiting };
      if (method === "issues.columns") return { columns: [] };
      return {};
    };
    await changes.greetBridge(call, { deviceId: "dev-1" });
    issuesPane.mountIssuesPane(document.querySelector("#issues"), {
      deviceId: "dev-1", projectId: "proj-1", projectName: "Build", projectKey: "dev-1|proj-1",
      defaultView: "dashboard", feed: () => ({ projects: [], workspaces: [], items: [] }), callRpc: call,
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
  }, hello);
  await page.locator('[data-dashboard-tab="backlog"]').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-dashboard-section="backlog"] [data-issue]').length === 4);
  await page.screenshot({ path: output });
}, { width, height, plugins: [deviceShim] });
