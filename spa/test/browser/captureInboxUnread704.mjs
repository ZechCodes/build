// Cache-only review evidence; run with CAPTURE_PHASE=before|after.
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/task704-evidence";
await mkdir(directory, { recursive: true });
const observations = {};
for (const { label, width, height } of [
  { label: "desktop", width: 1280, height: 800 }, { label: "phone", width: 390, height: 760 },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, `<div id="shell"><aside id="inbox-rail" aria-label="Inbox">
      <div class="inbox-head"><span class="dim">Inbox</span></div><div id="inbox-list"></div>
      </aside><main id="view"></main></div>`, { basePath });
    await loadBrowserModules(page, { rows: "src/core/inbox.js", projects: "src/core/inboxProjects.js",
      agents: "src/core/inboxProjectAgent.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js" }, basePath);
    await page.evaluate(async () => {
      const { cache, feed, rows, agents, projects } = window.__layoutModules;
      const deviceId = "unread-review", projectKey = `${deviceId}/build`;
      const project = { id: "build", name: "Build", entity_id: "owner", deviceId, projectKey };
      const now = Date.now();
      const digest = (id, over = {}) => ({ id, watched: true, unread_count: 0, working: false, ...over });
      const row = (id, roster) => ({ kind: "branch", entity_id: id, project_id: project.id, projectKey,
        deviceId, agents: roster, session_started_ms: now - 100_000, last_activity_ms: now });
      const address = (kind, entityId = "") => ({ deviceId, entityId, kind });
      await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
      await cache.writeCached(address("projects"), [project]);
      await cache.writeCached(address("agent-lineage-support"), { namesMakers: true });
      await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
      await cache.writeCached(address("project-agents", "owner"), {
        entity_id: "owner", pinned_agent_id: "pin", agents: [digest("pin", { topic: "Project coordinator" }),
          digest("middle", { created_by: "pin" })],
      });
      await cache.writeCached(address("workspaces"), [{ id: "worker", workspaceKey: `${deviceId}/worker`,
        deviceId, projectKey, project_id: project.id, name: "Worker with a new message", entity_id: "child",
        status: "ready", session_started_ms: now - 100_000, last_activity_ms: now }]);
      await cache.writeCached(address("row", "owner"), row("owner", [digest("pin")]));
      await cache.writeCached(address("row", "child"), row("child", [digest("worker", { created_by: "middle", unread_count: 1 })]));
      feed.subscribeFeed((view) => {
        const entries = [...agents.projectAgentEntries(view.projects, view.items, view.runs),
          ...rows.watchedWorkspaceEntries(view.workspaces, view.projects, view.items, view.runs)];
        document.querySelector("#inbox-list").innerHTML = entries.map((entry) => rows.inboxRowHtml(entry)).join("");
        window.__unread704 = { total: projects.projectsUnreadCount(entries, view.projects),
          projectUnread: !!document.querySelector(`[data-key="project-agent:${projectKey}"] .inbox-status-unread`) };
      });
      await feed.startFeed();
      if (innerWidth <= 900) document.body.classList.add("inbox-collapsed", "inbox-popover-open");
    });
    await page.evaluate(() => document.fonts.ready);
    observations[label] = await page.evaluate(() => window.__unread704);
    await page.locator("#inbox-rail").screenshot({ path: join(directory, `${phase}-inbox-${label}.png`) });
  }, { width, height });
}
await writeFile(join(directory, `${phase}-inbox.json`), `${JSON.stringify(observations, null, 2)}\n`);
console.log(JSON.stringify({ phase, observations }));
