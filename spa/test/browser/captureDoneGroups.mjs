// Capture the review image for #157: the mounted Dashboard's Done tab grouped
// by time, on a bridge that carries the user's session.
// Run from spa/: node test/browser/captureDoneGroups.mjs [output.png] [width] [height]
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./issueIdentityHarness.mjs";

const output = process.argv[2] || "/tmp/done-groups.png";
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
    const now = Date.now();
    const minutes = (count) => new Date(now - count * 60_000).toISOString();
    const finished = [
      [161, "Chat composer keeps its draft across reloads", minutes(4), "b41c0e7a9d2f"],
      [160, "Agent rail: unread dot clears on focus", minutes(11), "9e03d1c77ab4"],
      [158, "Board: drag a card between columns on touch", minutes(22), null],
      [156, "Files explorer remembers expanded folders", minutes(48), "51aa8c0f3e62"],
      [154, "Git pane: stage a hunk from the diff", minutes(95), "0c7b2e19d4f8"],
      [150, "Landing: scroll cue settles still", minutes(9 * 60), "e2d94b61a0c3"],
      [149, "Bridge: workspaces idle for 24 h notify the project agent", minutes(11 * 60), null],
    ].map(([number, title, doneAt, sha]) => ({
      id: `issue-${number}`, number, title, status: "done", state: "closed", assignee: null, labels: [],
      done_at: doneAt, links: { commits: sha ? [sha] : [] },
    }));
    const listed = {
      issues: finished,
      user_session: {
        session_started_ms: now - 2 * 60 * 60_000, last_activity_ms: now - 60_000,
        previous_session_ended_ms: now - 12 * 60 * 60_000, gap_ms: 6 * 60 * 60_000, now_ms: now,
      },
    };
    const call = async (method) => {
      if (method === "session.hello") return hello;
      if (method === "issues.list") return listed;
      if (method === "issues.columns") return { columns: [] };
      return {};
    };
    await changes.greetBridge(call, { deviceId: "dev-1" });
    issuesPane.mountIssuesPane(document.querySelector("#issues"), {
      deviceId: "dev-1", projectId: "proj-1", projectName: "Build", projectKey: "dev-1|proj-1",
      defaultView: "dashboard", feed: () => ({ projects: [], workspaces: [], items: [] }), callRpc: call,
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
  }, { ...hello, capabilities: [...hello.capabilities, "issues.doneSinceLeft"] });
  await page.locator('[data-dashboard-tab="done"]').click();
  await page.waitForFunction(() => document.querySelectorAll(".issue-dashboard-group-title").length >= 4);
  const titles = await page.locator(".issue-dashboard-group-title").allTextContents();
  console.log(titles.join(" | "));
  await page.screenshot({ path: output });
}, { width, height, plugins: [deviceShim] });
