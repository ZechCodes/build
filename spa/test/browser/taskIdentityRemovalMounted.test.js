import { readFile } from "node:fs/promises";
import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

// The normal suite replays a trace exported by the Rust AppState regression.
// The dedicated cross-stack check sets this path to a freshly generated trace,
// exercising the real agent.remove handler and subscription flush as well.
const tracePath = process.env.BUILD_TASK_AGENT_REMOVAL_TRACE || new URL("../fixtures/taskAgentRemoval.json", import.meta.url);

it("reconciles a real agent removal into mounted task, list and board identities while the workspace remains", async () => {
  const trace = JSON.parse(await readFile(tracePath, "utf8"));
  const task = trace.before.get.task;
  const removed = task.assignee.agent_id;
  const retained = Object.keys(task.identities).find((id) => id !== removed && task.identities[id].available);
  expect(retained, "the real trace also contains a live agent with no watched roster").toBeTruthy();
  const identity = task.identities[removed];
  const actorName = `${identity.workspace_name} · ${identity.name}`;
  const linkFor = (id) => `a[href*="agent=${id}"]`;
  const removedLink = linkFor(removed);
  const surfaces = [
    "#task .task-comment .task-entry-head", "#task .task-page-body", "#task .task-assignee-current",
    `#list [data-task="${task.id}"]`, `#board [data-task="${task.id}"]`,
  ];

  await withLayoutPage(async ({ page, basePath }) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await mountLayout(page, '<main id="task"></main><main id="list"></main><main id="board"></main>', {
      basePath, styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block} main{margin:12px}`,
    });
    await loadBrowserModules(page, {
      cache: "src/core/trackerCache.js", changes: "src/core/changeEvents.js",
      taskPage: "src/core/trackerTaskPage.js", tasksPane: "src/core/trackerTasksPane.js",
      references: "src/core/referenceIndex.js",
    }, basePath);
    await page.evaluate(async (trace) => {
      const { cache, changes, taskPage, tasksPane, references } = window.__layoutModules;
      const task = trace.before.get.task;
      const identity = task.identities[task.assignee.agent_id];
      const deviceId = "identity-removal-device";
      const projectId = task.project_id;
      const projectKey = `${deviceId}/${projectId}`;
      const feed = {
        projects: [{ id: projectId, name: "Build", projectKey }],
        workspaces: [{ id: identity.workspace_id, workspace_id: identity.workspace_id, name: identity.workspace_name, projectKey,
          deviceId, project_id: projectId }],
        items: [], // These agents are unwatched; missing roster data is not deletion.
      };
      // The app fills the reference index from this same feed
      // (core/referenceIndexFeed.js, #229); nothing here starts that feed.
      references.holdReferenceSources({ feed, tasks: {} });
      const replay = { phase: "before", calls: [], trace, deviceId, projectId, taskId: task.id, feed };
      const callRpc = async (method, params) => {
        replay.calls.push({ method, params, phase: replay.phase });
        if (method === "session.hello") return trace.greeting;
        if (method === "tasks.get" && params.task_id === task.id) return structuredClone(trace[replay.phase].get);
        if (method === "tasks.list") return structuredClone(trace[replay.phase].list);
        return {};
      };
      await changes.greetBridge(callRpc, { deviceId });
      await cache.writeTaskRecord(deviceId, projectId, task.id, cache.taskRecord(task, trace.before.get.timeline));
      await cache.writeTasksRecord(deviceId, projectId, trace.before.list);
      const common = { deviceId, projectId, projectKey, feed: () => feed, callRpc,
        catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {} };
      replay.page = taskPage.mountTaskPage(document.querySelector("#task"), { ...common, taskId: task.id });
      replay.list = tasksPane.mountTasksPane(document.querySelector("#list"), { ...common, view: "list" });
      replay.board = tasksPane.mountTasksPane(document.querySelector("#board"), { ...common, view: "board" });
      window.__removalReplay = replay;
      await changes.subscriptionsSettled();
    }, trace);

    for (const surface of surfaces) {
      await page.locator(`${surface} ${removedLink}`).first().waitFor();
    }
    expect(await page.locator(`#task .task-page-body ${linkFor(retained)}`).count()).toBe(1);
    expect(await page.evaluate(() => window.__removalReplay.feed.items)).toEqual([]);
    expect(await page.evaluate(() => window.__layoutModules.changes.bridgeCapabilities("identity-removal-device").changes.kinds)).toContain("tasks");

    // Only replay the actual bridge flush. The production subscriptions pull
    // new RPC results, write IndexedDB, and repaint on cache announcements.
    // No view refresh, feedMoved, or cache write is called from this test.
    await page.evaluate(async () => {
      const { changes } = window.__layoutModules;
      const replay = window.__removalReplay;
      await changes.subscriptionsSettled();
      replay.phase = "after";
      for (const event of replay.trace.events) changes.dispatchChangeEvent(event, replay.deviceId);
    });
    await expect.poll(() => page.evaluate(
      (selectors) => selectors.map((selector) => document.querySelectorAll(selector).length),
      surfaces.map((surface) => `${surface} ${removedLink}`),
    ), { timeout: 5_000 }).toEqual(surfaces.map(() => 0));
    expect(await page.locator(removedLink).count()).toBe(0);
    for (const selector of ["#task .task-comment", `#list [data-task="${task.id}"]`, `#board [data-task="${task.id}"]`]) {
      expect(await page.locator(selector).textContent()).toContain(actorName);
      expect(await page.locator(selector).locator(`[data-harness-icon="${identity.provider}"]`).count()).toBeGreaterThan(0);
    }
    expect(await page.locator(`#task .task-page-body ${linkFor(retained)}`).count()).toBe(1);
    expect(await page.evaluate(() => window.__removalReplay.feed.workspaces.length)).toBe(1);

    const reconciled = await page.evaluate(async (removed) => {
      const { cache } = window.__layoutModules;
      const { deviceId, projectId, taskId, calls } = window.__removalReplay;
      const detail = await cache.readTaskRecord(deviceId, projectId, taskId);
      const list = await cache.readTasksQueryRecord(deviceId, projectId, { project_id: projectId, state: "open" });
      return {
        detail: detail.task.identities[removed],
        list: list.tasks.find((task) => task.id === taskId).identities[removed],
        pulled: calls.filter((call) => call.phase === "after").map((call) => call.method),
      };
    }, removed);
    expect(reconciled.detail).toMatchObject({ name: identity.name, provider: identity.provider, available: false });
    expect(reconciled.list).toMatchObject({ name: identity.name, provider: identity.provider, available: false });
    expect(reconciled.pulled).toContain("tasks.get");
    expect(reconciled.pulled).toContain("tasks.list");
    for (const view of ["list", "board"]) {
      await page.locator(`#${view} [data-task="${task.id}"] [data-task-assign]`).click();
      const picker = page.locator("#task-assign-scrim [role='dialog']");
      await picker.waitFor({ state: "visible", timeout: 5_000 });
      await picker.locator("[data-assign-cancel]").click();
      await picker.waitFor({ state: "detached" });
    }
    expect(errors).toEqual([]);
    await page.evaluate(() => {
      window.__removalReplay.page.dispose();
      window.__removalReplay.list.dispose();
      window.__removalReplay.board.dispose();
      window.__layoutModules.changes.resetChangeEvents();
    });
  }, { plugins: [deviceShim] });
}, 30_000);
