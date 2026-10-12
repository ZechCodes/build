import { mountLayout } from "./layoutHarness.mjs";
import { loadChatOverviewModules } from "./chatOverviewSeed.mjs";

export async function mountModelBadges(page, basePath) {
  await mountLayout(page, `<div id="shell"><div id="view"><header id="toolbar">Build / #539</header>
    <div id="view-body"><main id="root"><div class="task-page"><div class="task-page-main" id="badge-task"></div></div></main>
    <aside id="agent-rail" aria-label="Agents"></aside></div><div id="console-region"></div></div></div>`, { basePath,
    styles: ".task-page{padding:20px;max-width:760px} #toolbar{padding:12px 20px}",
  });
  await loadChatOverviewModules(page, basePath, { task: "src/core/trackerTaskRender.js",
    assignee: "src/core/trackerAssignee.js" });
  await page.evaluate(async () => {
    const { cache, task, assignee, rail, app, feed: taskFeed, merge } = window.__layoutModules;
    app.App.devices = [{ id: "badge-device", name: "This computer", status: "online" }];
    app.App.selectedDeviceId = "badge-device";
    const agents = [
      { id: "badge-astra", name: "Navigation planner with a deliberately long name", provider: "codex_app_server", active_model: "gpt-6-astra", effort: "high" },
      { id: "badge-fable", name: "Implementer", provider: "claude_adk", model: "claude-fable-5-1" },
      { id: "badge-opus", name: "Reviewer", provider: "claude_adk", model: "claude-opus-5-5" },
      { id: "badge-grok", name: "Explorer", provider: "pi", model: "grok-4.7" },
      { id: "badge-unknown", name: "Custom agent", provider: "pi", model: "house-model" },
    ].map((agent, index) => ({ ...agent, ordinal: index + 1, state: "live", topic: agent.name, unread_count: 0 }));
    const address = { deviceId: "badge-device", entityId: "badge-run", kind: "row", sub: "" };
    await cache.writeCached(address, { kind: "workspace", workspace_id: "badge-workspace", entity_id: "badge-run", project_id: "badge-project", agents });
    await cache.writeCached({ deviceId: "badge-device", entityId: "", kind: "workspaces" }, [
      { id: "badge-workspace", project_id: "badge-project", entity_id: "badge-run", name: "navigation-plan" },
    ].map((workspace) => merge.stampWorkspace(workspace, "badge-device")));
    await cache.writeCached({ deviceId: "badge-device", entityId: "", kind: "projects" }, [
      { project_id: "badge-project", name: "Build" },
    ].map((project) => merge.stampProject(project, "badge-device")));
    const roster = (await cache.readCached(address)).value;
    const feed = { workspaces: [{ id: "badge-workspace", entity_id: "badge-run", projectKey: "badge-key", name: "navigation-plan" }],
      items: [{ ...roster, projectKey: "badge-key" }] };
    const groups = assignee.workspaceAgents(feed, "badge-key");
    const context = { deviceId: "badge-device", projectId: "badge-project", agentGroups: groups,
      agentLabels: assignee.agentLabels(groups), agentProviders: assignee.agentProviders(groups) };
    document.querySelector("#badge-task").innerHTML = `<h1>Navigation plan</h1>${task.timelineHtml(agents.map((agent, index) => ({
      key: `badge-comment-${index}`, type: "comment", actor: { kind: "agent", agent_id: agent.id },
      at: new Date().toISOString(), body: "The proposed navigation model keeps the current project in view.",
    })), context)}`;
    localStorage.setItem("build.rail.expanded", "1");
    await taskFeed.startFeed();
    window.__badgeRail = rail.mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "workspace", deviceId: "badge-device", projectId: "badge-project", workspaceId: "badge-workspace", entityId: "badge-run",
      call: async (method) => method === "models.list" ? { providers: [] } : { items: [] },
    });
  });
  await page.waitForSelector(".rail-overview-toggle");
  await page.locator(".rail-overview-toggle").click();
  await page.waitForSelector('[data-overview-agent="badge-astra"]');
}
