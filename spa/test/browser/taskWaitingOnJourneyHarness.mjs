import { loadBrowserModules } from "./layoutHarness.mjs";
import { mountWaitingOnFixture } from "./taskWaitingOnHarness.mjs";

// Script the fixture bridge's durable release and delivery replies. Production
// task actions, cache writes, task page and dashboard consume them together;
// the bridge's at-most-once service is covered by the inherited Rust tests.
export async function mountWaitingJourney(page, basePath) {
  await mountWaitingOnFixture(page, basePath);
  await loadBrowserModules(page, {
    pane: "src/core/trackerTasksPane.js", waitingCache: "src/core/taskWaitingOnCache.js",
  }, basePath);
  await page.evaluate(async () => {
    const fixture = window.__waiting;
    const { pane, waitingCache } = window.__layoutModules;
    const deviceId = "device-waiting", projectId = "proj-1", projectKey = `${deviceId}/${projectId}`;
    const unassigned = { ...structuredClone(fixture.current), id: "unassigned", number: 565,
      title: "Unassigned follow-up", status: "backlog", labels: ["keep-label"] };
    const retired = ["cancelled", "delivered"].map((state, index) => ({
      ...structuredClone(unassigned), id: state, number: 566 + index,
      deferred_assignment: { id: `saved-${state}`, state, target: { kind: "new_workspace" } },
    }));
    fixture.tasks.push(unassigned, ...retired);
    fixture.provisions = [];
    fixture.deliveries = [];
    fixture.notices = [];
    const publish = (task) => waitingCache.cacheTaskWaitingOnResponse(deviceId, projectId, { task: structuredClone(task) });
    const originalCall = fixture.call;
    fixture.call = async (method, params) => {
      if (method === "tasks.get") {
        fixture.calls.push({ method, params: structuredClone(params) });
        return { task: structuredClone(fixture.tasks.find((task) => task.id === params.task_id)), timeline: [] };
      }
      if (method === "tasks.update" || method === "tasks.close") {
        fixture.calls.push({ method, params: structuredClone(params) });
        const prerequisite = fixture.tasks.find((task) => task.id === params.task_id);
        if (method === "tasks.close") prerequisite.state = "closed";
        else Object.assign(prerequisite, params);
        const resolution = prerequisite.state === "closed" ? "close" : prerequisite.status === "done" ? "done" : null;
        for (const task of [fixture.current, unassigned, ...retired]) {
          const edge = task.dependencies.waiting_on.find((edge) => edge.task_id === prerequisite.id);
          Object.assign(edge, { state: prerequisite.state, status: prerequisite.status, resolved: Boolean(resolution), resolved_by: resolution });
          task.dependencies.blocked = task.dependencies.waiting_on.some((edge) => !edge.resolved);
          if (!task.dependencies.blocked && task.deferred_assignment?.state === "waiting") {
            fixture.provisions.push({ workspace_id: "journey-workspace", agent_id: "journey-agent" });
            fixture.deliveries.push({ id: task.deferred_assignment.id,
              closed_without_done_task_ids: task.dependencies.waiting_on.filter((edge) => edge.resolved_by === "close").map((edge) => edge.task_id),
              body: "Assess prerequisites closed without Done before proceeding; escalate unclear closures on the dependent task." });
            task.deferred_assignment.state = "delivered";
            task.assignee = { kind: "agent", agent_id: "journey-agent" };
            task.status = "in_progress";
          }
          if (!task.dependencies.blocked && task.id === unassigned.id) fixture.notices.push({ task_id: task.id, mentions_user: false });
          task.updated_at = new Date(Date.now() + fixture.calls.length * 1000).toISOString();
          await publish(task);
        }
        await publish(prerequisite);
        return { task: structuredClone(prerequisite) };
      }
      return originalCall(method, params);
    };
    for (const task of [unassigned, ...retired]) await publish(task);
    const host = document.createElement("section");
    host.id = "journey-dashboard";
    document.body.appendChild(host);
    fixture.dashboard = pane.mountTasksPane(host, { deviceId, projectId, projectKey, projectName: "Build",
      defaultView: "dashboard", callRpc: fixture.context.rpc,
      feed: () => ({ projects: [{ id: projectId, name: "Build", projectKey }],
        workspaces: [{ id: "journey-workspace", entity_id: "journey-run", projectKey, name: "Journey" }],
        items: [{ run_id: "journey-run", projectKey, agents: [{ id: "journey-agent", name: "Implementer", working: true }] }] }),
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate() {},
    });
  });
  await page.locator('#journey-dashboard [data-dashboard-tab="backlog"]').click();
  await page.locator('#journey-dashboard [data-backlog-group="blocked"] [data-task="unassigned"]').waitFor();
}
