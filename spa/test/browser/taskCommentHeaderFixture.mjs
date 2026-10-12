import { readFile } from "node:fs/promises";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

const answer = JSON.parse(await readFile(new URL("../../../fixtures/api/v1/tasks.get.json", import.meta.url), "utf8"));
export const LONG = '[data-comment-id="tc-sticky-long"]';
export const SHORT = '[data-comment-id="tc-sticky-short"]';
export const modes = [
  { label: "desktop", width: 1280, height: 800 },
  { label: "phone", width: 390, height: 844 },
].flatMap((mode) => ["light", "dark"].map((theme) => ({ ...mode, theme })));

function taskRecord() {
  const task = structuredClone(answer.result.task);
  task.number = 557;
  task.title = "Task comment: reply icon in a sticky header";
  task.body = "Keep the comment identity and reply action visible while reading a long comment.";
  const paragraph = "The comment header stays with the comment while the task history scrolls. Readers can see who wrote the update and reply without losing their place. ".repeat(3);
  const comment = (id, body, eligible = false) => ({
    type: "comment", id, body, author: task.assignee, created_at: "2026-10-12T12:00:00Z", attachments: [],
    ...(eligible ? { opinion: { verdict: "comment" } } : {}),
  });
  return { task, timeline: [
    comment("tc-sticky-before", "The earlier update remains in the timeline."),
    comment("tc-sticky-long", Array.from({ length: 18 }, (_, index) => `Paragraph ${index + 1}. ${paragraph}`).join("\n\n"), true),
    comment("tc-sticky-short", "A short eligible comment fits on one line at a desk.", true),
    comment("tc-sticky-after", Array.from({ length: 8 }, () => paragraph).join("\n\n")),
  ] };
}

export async function settleHeader(page) {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

export async function mountCommentHeaderPage(page, basePath, theme) {
  await mountLayout(page, '<div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div>', {
    basePath, styles: `@import url("${basePath}src/styles/tasks.css");
      @import url("${basePath}src/styles/surfaces.css");
      html,body{height:100%;margin:0} body{display:block} #tabbody{height:100%}`,
  });
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  await loadBrowserModules(page, { cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js" }, basePath);
  await page.evaluate(async (record) => {
    const { cache, taskPage } = window.__layoutModules;
    window.__stickyRecord = record;
    await cache.writeTasksRecord("sticky-device", "proj-1", { tasks: [record.task], columns: [] });
    await cache.writeTaskRecord("sticky-device", "proj-1", record.task.id, record);
    window.__stickyPage = taskPage.mountTaskPage(document.querySelector("#task-pane"), {
      deviceId: "sticky-device", projectId: "proj-1", projectKey: "sticky-device/proj-1", taskId: record.task.id,
      feed: () => ({ projects: [], workspaces: [], items: [] }), callRpc: () => new Promise(() => {}),
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
  }, taskRecord());
  await page.waitForFunction(() => document.querySelectorAll("#task-pane .task-comment").length === 4);
  await settleHeader(page);
}

export async function parkComment(page, selector, offset = 0) {
  await page.locator(selector).evaluate((row, offset) => {
    const host = document.querySelector("#task-pane");
    const head = host.querySelector(".task-page-head").getBoundingClientRect();
    host.scrollTop += row.getBoundingClientRect().top - head.bottom + offset;
  }, offset);
  await settleHeader(page);
}

export async function commentGeometry(page, selector) {
  return page.locator(selector).evaluate((row) => {
    const box = (node) => {
      if (!node) return null;
      const { top, right, bottom, left, width, height } = node.getBoundingClientRect();
      return { top, right, bottom, left, width, height };
    };
    const header = row.querySelector(".task-entry-head");
    const button = row.querySelector("[data-review-reply]");
    return { row: box(row), header: box(header), button: box(button),
      pageHead: box(document.querySelector(".task-page-head")),
      actor: box(header.querySelector("strong")),
      card: box(row.querySelector(".task-comment-card")),
      body: box(row.querySelector(".task-comment-body")), buttonInHeader: header.contains(button),
      icon: !!button?.querySelector("svg"), visibleText: button?.textContent.trim(),
      accessibleLabel: button?.getAttribute("aria-label"), viewport: { width: innerWidth, height: innerHeight },
      documentWidth: document.documentElement.scrollWidth,
      paneScrollLeft: document.querySelector("#task-pane").scrollLeft };
  });
}

export async function updateCommentTaskTitle(page, title) {
  await page.evaluate(async (title) => {
    const record = window.__stickyRecord;
    record.task.title = title;
    await window.__layoutModules.cache.writeTaskRecord("sticky-device", "proj-1", record.task.id, record);
  }, title);
  await page.waitForFunction((title) => document.querySelector(".task-page-title")?.textContent === title, title);
  await settleHeader(page);
}

export async function updateCommentTaskIdentity(page, name) {
  await page.evaluate(async (name) => {
    const record = window.__stickyRecord;
    record.task.identities[record.task.assignee.agent_id].name = name;
    await window.__layoutModules.cache.writeTaskRecord("sticky-device", "proj-1", record.task.id, record);
  }, name);
  await page.waitForFunction(({ selector, name }) => document.querySelector(`${selector} .task-entry-head strong`)?.textContent.includes(name), { selector: LONG, name: name.trim() });
  await settleHeader(page);
}
