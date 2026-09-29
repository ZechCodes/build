import { expect, it } from "vitest";
import answer from "../../../fixtures/api/v1/tasks.get.json";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

it.each(["list", "board"])("opens the mounted %s assignment picker before and after its cached target disappears", async (view) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<main id="tasks"></main>', {
      basePath, styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block}`,
    });
    await loadBrowserModules(page, {
      cache: "src/core/trackerCache.js", tasksPane: "src/core/trackerTasksPane.js",
    }, basePath);
    await page.evaluate(async ({ fixture, view }) => {
      const { cache, tasksPane } = window.__layoutModules;
      const task = structuredClone(fixture.result.task);
      const workspace = { id: "ws-3f2a91c4", workspace_id: "ws-3f2a91c4", name: "spa-flaky-tests", projectKey: "dev-1|proj-1" };
      const feed = { projects: [{ id: "proj-1", name: "Build", projectKey: "dev-1|proj-1" }], workspaces: [workspace], items: [] };
      await cache.writeTasksRecord("dev-1", "proj-1", { tasks: [task], columns: [] });
      const pane = tasksPane.mountTasksPane(document.querySelector("#tasks"), {
        deviceId: "dev-1", projectId: "proj-1", projectKey: "dev-1|proj-1", view,
        feed: () => feed, callRpc: () => new Promise(() => {}),
        catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
      });
      window.__assignmentMounted = { pane, feed, workspace };
    }, { fixture: answer, view });

    const press = page.locator("#tasks [data-task-assign]");
    await press.waitFor();
    for (const available of [true, false, true]) {
      await page.evaluate((available) => {
        const { pane, feed, workspace } = window.__assignmentMounted;
        feed.workspaces = available ? [workspace] : [];
        pane.feedMoved();
      }, available);
      expect(await page.locator("#tasks .task-assignee-link").count()).toBe(available ? 1 : 0);
      if (!available) {
        expect(await press.textContent()).toContain("spa-flaky-tests");
        expect(await press.locator("[data-harness-icon='codex_app_server']").count()).toBe(1);
      }
      await press.click();
      const picker = page.locator("#task-assign-scrim [role='dialog']");
      await picker.waitFor({ state: "visible", timeout: 5_000 });
      expect(await picker.locator("h3").textContent()).toBe(`Assign #${answer.result.task.number}`);
      await picker.locator("[data-assign-cancel]").click();
      await picker.waitFor({ state: "detached" });
    }
    await page.evaluate(() => window.__assignmentMounted.pane.dispose());
  }, { plugins: [deviceShim] });
}, 30_000);

it("opens each unwatched identity from mounted task, list, board and notice, then removes deleted routes", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await mountLayout(page, '<main id="task"></main><main id="list"></main><main id="board"></main><main id="notice"></main>', {
      basePath,
      styles: `@import url("${basePath}src/styles/tasks.css"); body{display:block} main{margin:12px}`,
    });
    await loadBrowserModules(page, {
      cache: "src/core/trackerCache.js",
      taskPage: "src/core/trackerTaskPage.js",
      tasksPane: "src/core/trackerTasksPane.js",
      notice: "src/core/trackerNotice.js",
      references: "src/core/referenceIndex.js",
    }, basePath);
    await page.evaluate(async (fixture) => {
      const { cache, taskPage, tasksPane, notice, references } = window.__layoutModules;
      const agentId = "agent-01K5ZQ8M4T0J7WQ2R6X3YB9C4E";
      const workspaceId = "ws-3f2a91c4";
      const actor = { kind: "agent", agent_id: agentId };
      const task = structuredClone(fixture.result.task);
      task.id = "task-mounted";
      task.body = `Ask @agent:${agentId} for the details.`;
      task.assignee = actor;
      task.links.workspace_ids = [workspaceId];
      const timeline = [
        { type: "comment", id: "tc-mounted", author: actor, body: "On it.", created_at: "2026-09-23T19:00:00Z" },
        { type: "event", id: "te-mounted", kind: "assigned", actor: { kind: "user" }, payload: { assignee: actor }, at: "2026-09-23T19:01:00Z" },
        { type: "event", id: "te-dispatched", kind: "dispatched", actor: { kind: "user" }, payload: { agent_id: agentId }, at: "2026-09-23T19:02:00Z" },
      ];
      const feed = { projects: [{ id: "proj-1", name: "Build", projectKey: "dev-1|proj-1" }],
        workspaces: [{ id: workspaceId, workspace_id: workspaceId, name: "spa-flaky-tests",
          projectKey: "dev-1|proj-1", entity_id: "run-unwatched", deviceId: "dev-1", project_id: "proj-1" }], items: [] };
      // The app fills the reference index from this same feed
      // (core/referenceIndexFeed.js, #229); nothing here starts that feed.
      references.holdReferenceSources({ feed, tasks: {} });
      const common = { deviceId: "dev-1", projectId: "proj-1", projectKey: "dev-1|proj-1",
        feed: () => feed, callRpc: async (method) => method === "tasks.get" ? { task, timeline }
          : method === "tasks.list" ? { tasks: [task], columns: [] } : {},
        catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {} };
      await cache.writeTaskRecord("dev-1", "proj-1", task.id, cache.taskRecord(task, timeline));
      await cache.writeTasksRecord("dev-1", "proj-1", { tasks: [task], columns: [] });
      const page = taskPage.mountTaskPage(document.querySelector("#task"), { ...common, taskId: task.id });
      const list = tasksPane.mountTasksPane(document.querySelector("#list"), { ...common, view: "list" });
      const board = tasksPane.mountTasksPane(document.querySelector("#board"), { ...common, view: "board" });
      const renderNotice = () => {
        document.querySelector("#notice").innerHTML = notice.taskNoticeLineHtml({
          task_id: task.id, number: task.number, action: "assigned",
          actor: { ...actor, identity: task.identities[agentId] },
          assignee: actor, assignee_identity: task.identities[agentId],
        }, { place: common, projectName: "Build", workspaces: feed.workspaces });
      };
      renderNotice();
      window.__identityMounted = { feed, page, list, board, renderNotice, references };
    }, answer);

    await page.waitForFunction(() => document.querySelector("#task .task-comment .task-entry-head a[href*='agent=']") &&
      document.querySelector("#list .task-assignee-link[href*='agent=']") &&
      document.querySelector("#board .task-assignee-link[href*='agent=']"));
    for (const selector of [
      "#task .task-comment .task-entry-head a", "#task .task-page-body a",
      "#task .task-assignee-current a", "#task .task-event a[href*='agent=']",
      "#list .task-assignee-link", "#board .task-assignee-link",
      "#notice .thread-task-by a", "#notice .thread-task-to a",
    ]) expect(await page.locator(selector).first().getAttribute("href"), selector).toContain("agent=");
    expect(await page.locator("#notice a.thread-task-number").getAttribute("href")).toContain("/tasks/task-mounted");
    expect(await page.locator("#list .task-assign[data-task-assign]").count()).toBe(1);
    expect(await page.locator("#board .task-assign[data-task-assign]").count()).toBe(1);

    await page.evaluate(() => {
      const mounted = window.__identityMounted;
      mounted.feed.workspaces = [];
      mounted.references.holdReferenceSources({ feed: { ...mounted.feed }, tasks: {} });
      mounted.page.feedMoved();
      mounted.list.feedMoved();
      mounted.board.feedMoved();
      mounted.renderNotice();
    });
    await page.waitForFunction(() => !document.querySelector("#task .task-comment .task-entry-head a") &&
      !document.querySelector("#task .task-page-body a") &&
      !document.querySelector("#task .task-assignee-current a") &&
      !document.querySelector("#list .task-assignee-link") &&
      !document.querySelector("#board .task-assignee-link") &&
      !document.querySelector("#notice a[href*='agent=']"));
    expect(await page.locator("#task .task-comment .task-entry-head").textContent()).toContain("spa-flaky-tests · Fix drag");
    expect(await page.locator("#notice a.thread-task-number").getAttribute("href")).toContain("/tasks/task-mounted");
    expect(errors).toEqual([]);
    await page.evaluate(() => {
      window.__identityMounted.page.dispose();
      window.__identityMounted.list.dispose();
      window.__identityMounted.board.dispose();
    });
  }, { plugins: [deviceShim] });
}, 30_000);
