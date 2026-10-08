// #411: replies identify their parent in a compact quote. Only a reader's
// deliberate press may jump there; ordinary cache paints keep their place.
import { expect, it } from "vitest";
import answer from "../../../fixtures/api/v1/tasks.get.json";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const PARENT = "tc-replies04";
const REPLY = "tc-replies26";
const replySelector = `[data-comment-id="${REPLY}"]`;
const quoteSelector = `${replySelector} .tracker-task-comment-reply[data-comment-jump="${PARENT}"]`;
const parentSelector = `[data-comment-id="${PARENT}"]`;
const PARENT_TEXT = "Keep this exact parent visible when the reply is opened.";
const PARENT_AGENT = "agent-replies-parent";
const SHELL = '<div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div>';
const styles = (basePath) => `@import url("${basePath}src/styles/tasks.css");
  @import url("${basePath}src/styles/surfaces.css");
  html,body{height:100%;margin:0} body{display:block} #tabbody{height:100%}`;

function replyRecord() {
  const task = structuredClone(answer.result.task);
  task.watched = true;
  task.read_through = "tc-replies29";
  task.identities[PARENT_AGENT] = { ...task.identities[task.assignee.agent_id], agent_id: PARENT_AGENT, name: "Parent agent" };
  const paragraph = "This cached task history explains the change in enough detail to keep the reader far away from the comment being answered. ".repeat(6);
  const timeline = Array.from({ length: 30 }, (_, index) => ({
    type: "comment", id: `tc-replies${String(index).padStart(2, "0")}`, author: task.assignee,
    body: `Comment ${index}.\n\n${Array.from({ length: 4 }, () => paragraph).join("\n\n")}`,
    created_at: "2026-10-07T12:00:00Z", attachments: [],
    author_context: { tokens: 12345, window: 1000000, compact_at: 200000, at: "2026-10-07T12:00:00Z" },
  }));
  timeline[4].body = `${PARENT_TEXT}\n\n${paragraph}`;
  timeline[4].author = { kind: "agent", agent_id: PARENT_AGENT };
  timeline[26].body = "The reply confirms that the parent remains easy to find without losing the current reading position.\n\n" + paragraph;
  timeline[26].reply_to = PARENT;
  return { task, timeline };
}

async function settle(page) {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  await page.waitForTimeout(100);
}

async function mountReplyPage(page, basePath) {
  await mountLayout(page, SHELL, { basePath, styles: styles(basePath) });
  await loadBrowserModules(page, {
    cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js", references: "src/core/referenceIndexFeed.js",
  }, basePath);
  await page.evaluate(async (record) => {
    const { cache, taskPage, references } = window.__layoutModules;
    const errors = [];
    window.addEventListener("error", (event) => errors.push(event.message));
    window.addEventListener("unhandledrejection", (event) => errors.push(String(event.reason)));
    const scope = { deviceId: "replies-device", projectId: "proj-1", taskId: record.task.id };
    const feed = { projects: [{ id: scope.projectId, deviceId: scope.deviceId,
      projectKey: `${scope.deviceId}/${scope.projectId}`, name: "Build" }], workspaces: [], items: [] };
    await cache.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [record.task], columns: [] });
    await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
    const stopIndex = references.feedReferenceIndex({ subscribeFeed: (listener) => { listener(feed); return () => {}; } });
    const host = document.querySelector("#task-pane");
    const scrollCalls = [];
    const scrollTo = host.scrollTo.bind(host);
    host.scrollTo = (...args) => { scrollCalls.push("scrollTo"); scrollTo(...args); };
    const scrollIntoView = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (...args) {
      scrollCalls.push("scrollIntoView");
      return scrollIntoView.apply(this, args);
    };
    const mounted = taskPage.mountTaskPage(host, {
      ...scope, projectKey: `${scope.deviceId}/${scope.projectId}`, feed: () => feed,
      callRpc: () => new Promise(() => {}), catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
    window.__replies = { record, scope, mounted, stopIndex, errors, scrollCalls };
  }, replyRecord());
  await page.waitForFunction(() => document.querySelectorAll("#task-pane .task-comment").length === 30);
  await settle(page);
}

async function parkAtReply(page) {
  await page.locator(replySelector).evaluate((row) => {
    const host = document.querySelector("#task-pane");
    const head = host.querySelector(".task-page-head").getBoundingClientRect();
    host.scrollTop += row.getBoundingClientRect().top - head.bottom - 32;
  });
  await settle(page);
}

async function saveReplyGeometry(page) {
  return page.evaluate(({ replySelector, quoteSelector }) => {
    const host = document.querySelector("#task-pane");
    const row = document.querySelector(replySelector);
    const quote = document.querySelector(quoteSelector);
    const paragraph = row.querySelector(".task-comment-body p");
    window.__replies.saved = { row, quote, paragraph };
    window.__replies.scrollCalls.length = 0;
    return { scrollTop: host.scrollTop, rowHeight: row.getBoundingClientRect().height, quoteHeight: quote.getBoundingClientRect().height,
      paragraphTop: paragraph.getBoundingClientRect().top };
  }, { replySelector, quoteSelector });
}

async function updateReplyCache(page, action) {
  await page.evaluate(async ({ action, parent, parentAgent }) => {
    const { record, scope } = window.__replies;
    if (action === "context") record.timeline.find((entry) => entry.id === parent).author_context = {
      tokens: 999999, window: 2000000, compact_at: 1500000, at: "2026-10-07T12:10:00Z",
    };
    if (action === "context") record.task.identities[parentAgent].name = "Parent agent with an updated and deliberately very long name ".repeat(8);
    if (action === "identity-reset") record.task.identities[parentAgent].name = "Parent agent";
    if (action === "append") record.timeline.push({ type: "comment", id: "tc-replies30", author: { kind: "user" },
      body: "Another cached comment arrives while the reader is looking at the reply.", created_at: "2026-10-07T12:11:00Z" });
    await window.__layoutModules.cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
  }, { action, parent: PARENT, parentAgent: PARENT_AGENT });
  if (action === "append") await page.locator('[data-comment-id="tc-replies30"]').waitFor({ state: "attached" });
  await settle(page);
}

async function expectReplyStill(page, before, label) {
  const after = await page.evaluate(({ replySelector, quoteSelector }) => {
    const row = document.querySelector(replySelector);
    const quote = document.querySelector(quoteSelector);
    const saved = window.__replies.saved;
    return { scrollTop: document.querySelector("#task-pane").scrollTop,
      rowHeight: row.getBoundingClientRect().height, quoteHeight: quote.getBoundingClientRect().height, paragraphTop: saved.paragraph.getBoundingClientRect().top,
      sameRow: row === saved.row, sameQuote: quote === saved.quote,
      sameParagraph: saved.paragraph.isConnected && row.contains(saved.paragraph), scrollCalls: window.__replies.scrollCalls };
  }, { replySelector, quoteSelector });
  console.log(`${label}: ${JSON.stringify({ before, after })}`);
  expect(Math.abs(after.rowHeight - before.rowHeight), "parent metadata changed reply height").toBeLessThanOrEqual(1);
  expect(Math.abs(after.quoteHeight - before.quoteHeight), "parent context changed quote height").toBeLessThanOrEqual(1);
  expect(Math.abs(after.paragraphTop - before.paragraphTop), "visible reply text moved").toBeLessThanOrEqual(1);
  expect([after.sameRow, after.sameQuote, after.sameParagraph]).toEqual([true, true, true]);
  expect(after.scrollCalls, "cache paint performed navigation").toEqual([]);
}

async function expectParentRevealed(page) {
  await page.waitForFunction((selector) => {
    const host = document.querySelector("#task-pane");
    const row = document.querySelector(selector);
    const box = row.getBoundingClientRect();
    return row.classList.contains("task-comment-target") &&
      box.top < host.getBoundingClientRect().bottom && box.bottom > host.querySelector(".task-page-head").getBoundingClientRect().bottom;
  }, parentSelector);
  expect(await page.locator(parentSelector).evaluate((row) => getComputedStyle(row.querySelector(".task-comment-card")).outlineStyle)).toBe("solid");
  expect(await page.evaluate(() => window.__replies.scrollCalls.length)).toBeGreaterThan(0);
}

for (const [label, width, height] of [["phone", 390, 844], ["desktop", 1440, 900]]) {
  it(`keeps the ${label} reply compact through cache paints and reveals its parent on click or keyboard press`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReplyPage(page, basePath);
      expect(await page.locator(quoteSelector).count(), "reply needs an actionable parent quote").toBe(1);
      expect(await page.locator(quoteSelector).textContent()).toContain("Parent agent");
      expect(await page.locator(quoteSelector).textContent()).toContain(PARENT_TEXT);
      expect(await page.locator(quoteSelector).textContent()).not.toContain(PARENT);
      await parkAtReply(page);
      const before = await saveReplyGeometry(page);
      expect(before.scrollTop).toBeGreaterThan(1000);
      expect(before.quoteHeight, "parent quote must stay compact").toBeLessThan(140);
      expect(await page.locator(parentSelector).evaluate((row) => row.getBoundingClientRect().bottom < 0)).toBe(true);
      await captureLayout(page, `task-411-${label}-reply.png`);
      for (const action of ["context", "append"]) {
        await updateReplyCache(page, action);
        if (action === "context") expect(await page.locator(quoteSelector).textContent()).toContain("updated and deliberately");
        await expectReplyStill(page, before, `${label}/${action}`);
      }
      // The stress label proves clipping above. Review captures use the
      // ordinary identity so the arrival mark and parent body remain clear.
      await updateReplyCache(page, "identity-reset");
      for (const gesture of ["click", "Enter", "Space"]) {
        await parkAtReply(page);
        await page.evaluate(() => { window.__replies.scrollCalls.length = 0; });
        const quote = page.locator(quoteSelector);
        if (gesture === "click") await quote.click();
        else await quote.press(gesture);
        await expectParentRevealed(page);
        if (gesture === "click") await captureLayout(page, `task-411-${label}-parent.png`);
        const parentTop = await page.locator("#task-pane").evaluate((host) => host.scrollTop);
        expect(parentTop).toBeLessThan(before.scrollTop - 1000);
      }
      expect(await page.evaluate(() => window.__replies.errors)).toEqual([]);
      await page.evaluate(() => { window.__replies.mounted.dispose(); window.__replies.stopIndex(); });
    }, { width, height, plugins: [deviceShim] });
  }, 60_000);
}
