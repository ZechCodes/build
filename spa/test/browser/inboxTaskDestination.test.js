import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const body = readFileSync(new URL("../../index.html", import.meta.url), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

it("opens assigned and unassigned cached inbox tasks and marks only the clicked row current", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, body, { basePath });
    await loadBrowserModules(page, { app: "src/app.js", cache: "src/core/localCache.js",
      tracker: "src/core/trackerCache.js", feed: "src/core/taskFeed.js", inbox: "src/core/inboxView.js" }, basePath);
    await page.evaluate(async () => {
      const { app, cache, tracker, feed, inbox } = window.__layoutModules;
      const deviceId = "inbox-device";
      const projectId = "p1";
      const projectKey = `${deviceId}/${projectId}`;
      const now = new Date().toISOString();
      const project = { id: projectId, project_id: projectId, deviceId, projectKey, name: "Build", entity_id: "project-run" };
      const workspace = { id: "w1", project_id: projectId, deviceId, projectKey, workspaceKey: `${deviceId}/w1`,
        name: "Task workspace", entity_id: "workspace-run", status: "ready", managed: true, directories: [] };
      const tasks = [
        { id: "assigned", number: 1, title: "Assigned task", assignee: { kind: "agent", agent_id: "a1" },
          identities: { a1: { agent_id: "a1", available: true, workspace_id: "w1", name: "Worker" } } },
        { id: "unassigned", number: 2, title: "Unassigned task", assignee: null, identities: {} },
      ].map((task) => ({ ...task, project_id: projectId, state: "open", status: "in_progress", watched: true,
        body: "", labels: [], priority: "none", attachments: [], trackers: [],
        links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
        created_at: now, updated_at: now }));
      app.App.devices = [{ id: deviceId, name: "Fixture machine" }];
      app.App.route = { name: "inbox" };
      app.App.gated = false;
      app.App.deviceFilter = null;
      await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [project]);
      await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, [workspace]);
      await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { items: [], runs: [] });
      await tracker.writeTasksRecord(deviceId, projectId, tracker.tasksRecord(tasks, []));
      for (const task of tasks) await tracker.writeTaskRecord(deviceId, projectId, task.id, tracker.taskRecord(task, []));
      history.replaceState(null, "", "#/inbox");
      app.initRouter();
      inbox.mountInboxList();
      await feed.startFeed();
    });
    const assigned = page.locator('[data-key="tracker_task:assigned"]');
    const unassigned = page.locator('[data-key="tracker_task:unassigned"]');
    await assigned.waitFor();
    await unassigned.waitFor();
    await assigned.click();
    await page.waitForFunction(() => location.hash === "#/device/inbox-device/project/p1/workspace/w1/tasks/assigned?agent=a1");
    expect(await page.locator("#inbox-list .inbox-entry.active").getAttribute("data-key")).toBe("tracker_task:assigned");
    await unassigned.click();
    await page.waitForFunction(() => location.hash === "#/device/inbox-device/project/p1/tasks/unassigned");
    expect(await page.locator("#inbox-list .inbox-entry.active").getAttribute("data-key")).toBe("tracker_task:unassigned");
  });
}, 60_000);
