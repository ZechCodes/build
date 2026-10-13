import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

/** A fixture bridge with the real greeting authority, cache and task-page code.
 * Only the authenticated device recovery lifecycle is replaced by deviceShim. */
export async function mountWaitingOnFixture(page, basePath, { theme = "light", assigned = false, failed = false, old = false, cachedOnly = false } = {}) {
  await mountLayout(page, '<main id="task"></main>', {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block;margin:0;padding:12px} #task{height:calc(100dvh - 24px);overflow:auto;max-width:1240px;margin:auto}`,
  });
  await loadBrowserModules(page, {
    cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js",
    contexts: "src/core/deviceContexts.js", events: "src/core/changeEvents.js",
  }, basePath);
  await page.evaluate(async ({ theme, assigned, failed, old, cachedOnly }) => {
    document.documentElement.dataset.theme = theme;
    const { cache, taskPage, contexts, events } = window.__layoutModules;
    const deviceId = "device-waiting";
    const projectId = "proj-1";
    const projectKey = `${deviceId}/${projectId}`;
    const task = (id, number, title, extra = {}) => ({
      id, project_id: projectId, number, title, body: "Review the prerequisite before starting this change.",
      state: "open", status: "ready", labels: [], priority: "none", assignee: null,
      links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
      created_by: { kind: "user" }, updated_at: "2026-10-12T12:00:00Z", ...extra,
    });
    const digest = (task, resolved_by = null) => ({
      task_id: task.id, number: task.number, title: task.title, state: task.state,
      status: task.status, resolved: Boolean(resolved_by), resolved_by,
    });
    const blocker = task("prerequisite", 12, "Finish shared cache");
    const closed = task("closed-prerequisite", 18, "Discard old approach", { state: "closed", status: "backlog" });
    const done = task("done-prerequisite", 19, "Review the contract", { status: "done" });
    const hostile = task("hostile-title", 23, '<img src=x onerror="window.__titleExecuted=true">');
    const candidate = task("candidate", 24, "Add integration coverage");
    const foreign = task("foreign", 99, "Other project task", { project_id: "proj-2" });
    const dependencies = { blocked: true, waiting_on: [digest(blocker), digest(closed, "close"), digest(done, "done")],
      blocks: [digest(task("dependent", 31, "Follow-up task"))], blocks_count: 2, blocks_next_cursor: "next-blocks" };
    const current = task("task-waiting", 564, "Edit task prerequisites", {
      dependencies,
      ...(assigned ? { assignee: { kind: "project_agent" }, deferred_assignment: {
        id: "saved-assignment-exact", target: { kind: "project_agent" }, state: failed ? "failed" : "waiting",
        ...(failed ? { error: "Could not start the saved assignment" } : {}),
      } } : {}),
    });
    const tasks = [current, blocker, closed, done, hostile, candidate, foreign];
    const fixture = { tasks, current, calls: [], navigations: [], refusal: null, readsHeld: cachedOnly, old };
    fixture.call = async (method, params) => {
      fixture.calls.push({ method, params: structuredClone(params || {}) });
      if (method === "session.hello") return { api_version: "3.21.0", capabilities: fixture.old ? [] : ["tasks.waitingOn"], push_events: false };
      if (method === "tasks.get") {
        if (fixture.readsHeld) return new Promise(() => {});
        return { task: structuredClone(fixture.current), timeline: [] };
      }
      if (method === "tasks.list") return { tasks: structuredClone(tasks), columns: [] };
      if (method === "tasks.waiting_on.blocks") return {
        blocks: params.cursor ? [digest(task("dependent-page-two", 32, "Second reverse page"))] : dependencies.blocks,
        ...(params.cursor ? {} : { next_cursor: "next-blocks" }),
      };
      if (method === "tasks.waiting_on.add" || method === "tasks.waiting_on.remove") {
        if (fixture.refusal) throw Object.assign(new Error(fixture.refusal), { code: "conflict" });
        if (method.endsWith(".add")) {
          for (const id of params.waiting_on) {
            const target = tasks.find((entry) => entry.id === id);
            if (!fixture.current.dependencies.waiting_on.some((entry) => entry.task_id === id)) {
              fixture.current.dependencies.waiting_on.push(digest(target));
            }
          }
        } else fixture.current.dependencies.waiting_on = fixture.current.dependencies.waiting_on.filter((entry) => !params.waiting_on.includes(entry.task_id));
        fixture.current.dependencies.blocked = fixture.current.dependencies.waiting_on.some((entry) => !entry.resolved);
        if (!fixture.current.dependencies.blocked && fixture.current.deferred_assignment) fixture.current.deferred_assignment.state = "queued";
        fixture.current.updated_at = new Date().toISOString();
        return { task: structuredClone(fixture.current) };
      }
      if (method === "tasks.assign") {
        if (fixture.refusal) throw Object.assign(new Error(fixture.refusal), { code: "conflict" });
        if (params.retry_deferred_id) fixture.current.deferred_assignment.state = "queued";
        else fixture.current.deferred_assignment = { id: "new-saved-assignment", target: params.assignee, state: "waiting" };
        return { task: structuredClone(fixture.current), dispatch: null };
      }
      return {};
    };
    const context = contexts.adoptDeviceSession({ deviceId, call: fixture.call, close() {} });
    fixture.greet = async (old) => {
      fixture.old = old;
      const authority = contexts.greetingInFlight(context);
      await events.greetBridge(fixture.call, { deviceId, isCurrent: authority.current, install(selection) {
        const adapter = selection.create(fixture.call);
        contexts.adoptBridgeSelection(context, selection, adapter, authority);
        return adapter;
      } });
    };
    await fixture.greet(old);
    await cache.writeTaskRecord(deviceId, projectId, current.id, { task: structuredClone(current), timeline: [] });
    await cache.writeTasksRecord(deviceId, projectId, { tasks: structuredClone(tasks), columns: [] });
    fixture.writeCurrent = () => cache.writeTaskRecord(deviceId, projectId, current.id, { task: structuredClone(fixture.current), timeline: [] });
    fixture.mounted = taskPage.mountTaskPage(document.querySelector("#task"), {
      deviceId, projectId, projectKey, taskId: current.id, callRpc: context.rpc,
      feed: () => ({ projects: [{ id: projectId, name: "Build", projectKey }], workspaces: [], items: [] }),
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
      navigate: (url) => fixture.navigations.push(url),
    });
    window.__waiting = fixture;
  }, { theme, assigned, failed, old, cachedOnly });
  await page.locator(".task-page-title").waitFor({ timeout: 5000 });
}
