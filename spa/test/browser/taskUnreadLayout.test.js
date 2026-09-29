import { expect, it } from "vitest";
import { captureLayout, mountLayout, loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const fixture = (readThrough = "tc-0016") => {
  const timeline = [{
    type: "event", id: "te-0000", kind: "created", at: "2026-09-23T12:00:00Z",
    actor: { kind: "user" }, payload: {},
  }];
  for (let number = 1; number <= 24; number += 1) {
    timeline.push({
      type: "comment",
      id: `tc-${String(number).padStart(4, "0")}`,
      task_id: "layout-task",
      author: number % 3 ? { kind: "agent", id: "layout-agent" } : { kind: "user" },
      body: `Comment ${number}. ${"A short discussion of this task. ".repeat(2)}`,
      created_at: `2026-09-23T12:${String(number).padStart(2, "0")}:00Z`,
    });
  }
  return {
    task: {
      id: "layout-task", project_id: "layout-project", number: 99,
      title: "Task comments with unread activity", body: "A reader returns to this task after several replies.",
      state: "open", status: "in_progress", labels: [], priority: "none", assignee: null,
      links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
      created_by: { kind: "user" }, created_at: "2026-09-23T12:00:00Z", updated_at: "2026-09-23T12:24:00Z",
      read_through: readThrough, watched: true, trackers: [],
    },
    timeline,
  };
};

const mountTaskFixture = async (page, basePath, { width = "100vw", height = "100vh", readThrough, shell = false } = {}) => {
  const task = '<main class="task-surface" id="task-layout"></main>';
  const markup = shell ? `<div id="shell"><aside id="inbox-rail" aria-label="Inbox">Inbox</aside>
    <div id="view"><div id="toolbar">Task</div><div id="view-body"><main id="root" class="surface"><div id="tabbody" class="flush">${task}</div></main>
    <aside id="agent-rail" aria-label="Chat"><div class="rail-panel"><div class="rail-body">Chat open</div></div></aside></div><div id="console-region"></div></div></div>` : task;
  await mountLayout(page, markup, {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css");
      #task-layout{height:${height};width:${width};max-width:none}
      ${shell ? "#shell{height:100%;box-sizing:border-box} #root{min-height:0} #tabbody{height:100%;box-sizing:border-box}" : "body{display:block;margin:0;width:100vw;height:100vh}"}`,
  });
  if (shell && (await page.evaluate(() => innerWidth)) <= 900) {
    await page.evaluate(() => {
      document.body.classList.add("inbox-collapsed");
      document.querySelector("#agent-rail").classList.add("rail-collapsed");
      document.querySelector(".rail-panel").setAttribute("aria-hidden", "true");
    });
  }
  await loadBrowserModules(page, {
    taskRender: "src/core/trackerTaskRender.js",
    timeline: "src/core/trackerTimeline.js",
    unread: "src/core/trackerUnread.js",
    marker: "src/core/unreadAnchor.js",
    pill: "src/core/newMessagesPill.js",
  }, basePath);
  await page.evaluate((answer) => {
    const host = document.querySelector("#task-layout");
    const { taskRender, timeline, unread, marker, pill } = window.__layoutModules;
    const rows = timeline.timelineRows(answer.timeline);
    const unreadFrom = marker.createUnreadMarker(() => {}, unread.taskUnreadRules)
      .update(unread.taskUnreadReading(rows, answer.task.read_through));
    host.innerHTML = taskRender.taskPageHtml(answer.task, {
      rows, unreadFrom, columns: [
        { id: "backlog", name: "Backlog" }, { id: "in_progress", name: "In progress" },
      ],
      agentLabels: {}, agentProviders: {}, projectName: "Build",
      links: [], watch: null, draft: "", labelsDraft: "", busy: false,
      sending: false, attachable: false, hasFiles: false,
    });
    const beforeHeight = host.scrollHeight;
    window.__layoutPill = pill.mountNewMessagesPill(host, { targetSelector: ".task-unread-line" });
    window.__layoutPill.sync();
    window.__layoutHeights = { before: beforeHeight, after: host.scrollHeight };
  }, fixture(readThrough));
};

for (const { label, width, height } of [
  { label: "desktop", width: 1440, height: 800 },
  { label: "mobile", width: 390, height: 760 },
]) {
  it(`shows the task unread line and pill, then jumps on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountTaskFixture(page, basePath, { shell: true, width: "100%", height: "100%" });

      await page.waitForSelector(".task-comment", { state: "visible" });
      await page.waitForSelector(".thread-unread-line", { state: "attached" });
      await page.waitForSelector(".new-messages-pill", { state: "visible" });
      const beforeScroll = await page.evaluate(() => ({
        page: document.scrollingElement.scrollTop, shell: document.querySelector("#shell").scrollTop,
        shellOverflow: getComputedStyle(document.querySelector("#shell")).overflowY,
      }));
      expect(beforeScroll).toEqual({ page: 0, shell: 0, shellOverflow: "clip" });
      expect(await page.locator(".task-comment").count()).toBe(24);
      const overlay = await page.evaluate(() => {
        const host = document.querySelector("#task-layout").getBoundingClientRect();
        const pill = document.querySelector(".new-messages-pill").getBoundingClientRect();
        const dock = document.querySelector(".new-messages-dock").getBoundingClientRect();
        return {
          heights: window.__layoutHeights, hostBottom: host.bottom,
          pillBottom: pill.bottom, pillHeight: pill.height, dockHeight: dock.height,
        };
      });
      expect(overlay.heights.after).toBe(overlay.heights.before);
      expect(overlay.dockHeight).toBe(0);
      expect(overlay.pillHeight).toBeGreaterThanOrEqual(30);
      expect(overlay.pillBottom).toBeLessThanOrEqual(overlay.hostBottom);
      expect(overlay.pillBottom).toBeGreaterThan(overlay.hostBottom - 100);
      await captureLayout(page, `task-unread-${label}-before.png`);

      await page.locator(".new-messages-pill").click();
      await page.waitForFunction(() => {
        const host = document.querySelector("#task-layout");
        const firstUnread = document.querySelector("#comment-tc-0017");
        const pill = document.querySelector(".new-messages-pill");
        if (!host || !firstUnread || !pill) return false;
        const bounds = host.getBoundingClientRect();
        const row = firstUnread.getBoundingClientRect();
        return Math.abs(document.querySelector(".task-unread-line").getBoundingClientRect().top - bounds.top) < 40
          && row.top >= bounds.top && row.top < bounds.bottom && !pill.getClientRects().length;
      });
      const after = await page.evaluate(() => {
        const host = document.querySelector("#task-layout");
        const firstUnread = document.querySelector("#comment-tc-0017");
        const pill = document.querySelector(".new-messages-pill");
        const bounds = host.getBoundingClientRect();
        const row = firstUnread.getBoundingClientRect();
        return {
          firstUnreadTop: row.top, surfaceTop: bounds.top, surfaceBottom: bounds.bottom,
          pillVisible: Boolean(pill?.getClientRects().length), scrollTop: host.scrollTop,
          pageScrollTop: document.scrollingElement.scrollTop, shellScrollTop: document.querySelector("#shell").scrollTop,
        };
      });
      expect(after.firstUnreadTop, JSON.stringify(after)).toBeGreaterThanOrEqual(after.surfaceTop);
      expect(after.firstUnreadTop, JSON.stringify(after)).toBeLessThan(after.surfaceBottom);
      expect(after.pillVisible, JSON.stringify(after)).toBe(false);
      expect(after.pageScrollTop).toBe(0);
      expect(after.shellScrollTop).toBe(0);
      await captureLayout(page, `task-unread-${label}-after.png`);
      await page.evaluate(() => window.__layoutPill.dispose());
    }, { width, height });
  }, 30_000);
}

it("updates the pill when only the task panel changes width", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountTaskFixture(page, basePath, { width: "900px", height: "550px", readThrough: "tc-0002" });
    const initial = await page.evaluate(() => ({
      hostBottom: document.querySelector("#task-layout").getBoundingClientRect().bottom,
      lineTop: document.querySelector(".task-unread-line").getBoundingClientRect().top,
      pillVisible: Boolean(document.querySelector(".new-messages-pill")?.getClientRects().length),
    }));
    expect(initial.lineTop, JSON.stringify(initial)).toBeLessThan(initial.hostBottom);
    await page.waitForFunction(() => {
      const host = document.querySelector("#task-layout");
      const line = document.querySelector(".task-unread-line");
      const pill = document.querySelector(".new-messages-pill");
      return line?.getBoundingClientRect().top < host?.getBoundingClientRect().bottom && !pill?.getClientRects().length;
    });
    const windowWidth = await page.evaluate(() => innerWidth);

    await page.evaluate(() => { document.querySelector("#task-layout").style.width = "300px"; });
    const narrow = await page.evaluate(() => ({
      hostBottom: document.querySelector("#task-layout").getBoundingClientRect().bottom,
      lineTop: document.querySelector(".task-unread-line").getBoundingClientRect().top,
      pillVisible: Boolean(document.querySelector(".new-messages-pill")?.getClientRects().length),
    }));
    expect(narrow.lineTop, JSON.stringify(narrow)).toBeGreaterThanOrEqual(narrow.hostBottom);
    await page.waitForFunction(() => {
      const host = document.querySelector("#task-layout");
      const line = document.querySelector(".task-unread-line");
      const pill = document.querySelector(".new-messages-pill");
      return line?.getBoundingClientRect().top >= host?.getBoundingClientRect().bottom && Boolean(pill?.getClientRects().length);
    });
    expect(await page.evaluate(() => innerWidth)).toBe(windowWidth);

    await page.evaluate(() => { document.querySelector("#task-layout").style.width = "900px"; });
    await page.waitForFunction(() => {
      const host = document.querySelector("#task-layout");
      const line = document.querySelector(".task-unread-line");
      const pill = document.querySelector(".new-messages-pill");
      return line?.getBoundingClientRect().top < host?.getBoundingClientRect().bottom && !pill?.getClientRects().length;
    });
    await page.evaluate(() => window.__layoutPill.dispose());
  }, { width: 1200, height: 800 });
}, 30_000);
