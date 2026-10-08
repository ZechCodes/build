// #410: metadata and cache repainting must leave a reader inside a long
// comment. These checks run production mounts, cache subscribers and styles
// in Chromium; a row which was detached and reinserted also fails identity.
import { expect, it } from "vitest";
import answer from "../../../fixtures/api/v1/tasks.get.json";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const READING_ROW = "comment-tc-reading-18";
const SHELL = '<div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div>';
const styles = (basePath) => `@import url("${basePath}src/styles/tasks.css");
  @import url("${basePath}src/styles/surfaces.css");
  html,body{height:100%;margin:0} body{display:block} #tabbody{height:100%}`;

function longRecord() {
  const task = structuredClone(answer.result.task);
  const paragraph = "The reader is halfway through this detailed comment, following a long explanation of the change and its consequences. ".repeat(9);
  return { task, timeline: Array.from({ length: 36 }, (_, index) => ({
    type: "comment", id: `tc-reading-${index}`, author: task.assignee,
    body: index % 3 ? `Short comment ${index}.` : `Comment ${index}: see #411.\n\n${Array.from({ length: 8 }, () => paragraph).join("\n\n")}`,
    created_at: "2026-10-07T12:00:00Z",
    author_context: { tokens: 12345, window: 1000000, compact_at: 200000, at: "2026-10-07T12:00:00Z" },
    attachments: index % 3 ? [] : [{ name: `shot-${index}.png`, path: `/store/shot-${index}.png`, mime: "image/png", size: 1 }],
    ...(index === 18 ? { opinion: { snapshot_id: "snapshot-reading", verdict: "approve" } } : {}),
  })) };
}

async function mountReadingPage(page, basePath, commentId = null) {
  await mountLayout(page, SHELL, { basePath, styles: styles(basePath) });
  await loadBrowserModules(page, {
    cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js",
    references: "src/core/referenceIndexFeed.js", review: "src/core/taskReviewCache.js",
    ui: "src/core/localUiState.js", uiStore: "src/core/localUiStore.js",
  }, basePath);
  await page.evaluate(async ({ record, commentId }) => {
    const { cache, taskPage, references } = window.__layoutModules;
    const scope = { deviceId: "reading-device", projectId: "proj-1", taskId: record.task.id };
    const feed = { projects: [{ id: scope.projectId, deviceId: scope.deviceId,
      projectKey: `${scope.deviceId}/${scope.projectId}`, name: "Build" }], workspaces: [], items: [] };
    await cache.writeTasksRecord(scope.deviceId, scope.projectId, { tasks: [record.task], columns: [] });
    await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
    const stopIndex = references.feedReferenceIndex({ subscribeFeed: (listener) => { listener(feed); return () => {}; } });
    // Only attachment bytes use this stand-in RPC. Task records, reference
    // lists, reviews and drafts reach the page through their real stores.
    const canvas = new OffscreenCanvas(300, 180);
    canvas.getContext("2d").fillRect(0, 0, 300, 180);
    const bytes = new Uint8Array(await (await canvas.convertToBlob({ type: "image/png" })).arrayBuffer());
    const attachment = { mime: "image/png", size: bytes.length, offset: 0, content_b64: btoa(String.fromCharCode(...bytes)) };
    const mounted = taskPage.mountTaskPage(document.querySelector("#task-pane"), {
      ...scope, projectKey: `${scope.deviceId}/${scope.projectId}`, commentId, feed: () => feed,
      callRpc: (method) => method === "tasks.attachment" ? Promise.resolve(attachment) : new Promise(() => {}),
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
    window.__reading = { record, scope, mounted, stopIndex };
  }, { record: longRecord(), commentId });
  await page.waitForFunction(() => {
    const images = [...document.querySelectorAll("#task-pane img.thread-attachment-image")];
    return document.querySelectorAll("#task-pane .task-comment").length === 36 &&
      images.length === 12 && images.every((image) => image.complete && image.naturalHeight === 180);
  });
  await settle(page);
}

async function settle(page) {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  await page.waitForTimeout(100);
}

async function parkReader(page) {
  await page.evaluate((id) => {
    const host = document.querySelector("#task-pane");
    const row = document.getElementById(id);
    const body = row.querySelector(".task-comment-body");
    host.scrollTop += body.getBoundingClientRect().top + body.offsetHeight / 2 - host.getBoundingClientRect().top - host.clientHeight / 2;
  }, READING_ROW);
  await settle(page);
  return page.evaluate((id) => {
    const host = document.querySelector("#task-pane");
    const row = document.getElementById(id);
    window.__reading.saved = { row, body: row.querySelector(".task-comment-body"), image: row.querySelector("img") };
    window.__reading.scrolls = [];
    if (!window.__reading.scrollListener) {
      window.__reading.scrollListener = () => window.__reading.scrolls.push(host.scrollTop);
      host.addEventListener("scroll", window.__reading.scrollListener);
    }
    return host.scrollTop;
  }, READING_ROW);
}

async function measure(page, before, label) {
  await settle(page);
  const result = await page.evaluate(({ id, before }) => {
    const host = document.querySelector("#task-pane");
    const row = document.getElementById(id);
    const saved = window.__reading.saved;
    return { before, after: host.scrollTop, delta: host.scrollTop - before,
      sameRow: saved.row === row, sameBody: saved.body === row.querySelector(".task-comment-body"),
      sameImage: saved.image === row.querySelector("img"), scrolls: window.__reading.scrolls };
  }, { id: READING_ROW, before });
  console.log(`${label}: ${JSON.stringify(result)}`);
  expect.soft(Math.abs(result.delta), `${label}: scrollTop moved`).toBeLessThanOrEqual(1);
  expect.soft(result.scrolls.every((top) => Math.abs(top - before) <= 1), `${label}: transient scroll jump`).toBe(true);
  expect.soft(result.sameRow, `${label}: reading row replaced`).toBe(true);
  expect.soft(result.sameBody, `${label}: reading body replaced`).toBe(true);
  expect.soft(result.sameImage, `${label}: loaded attachment replaced`).toBe(true);
}

async function updateRecord(page, action) {
  await page.evaluate(async (action) => {
    const { cache } = window.__layoutModules;
    const { record, scope } = window.__reading;
    if (action === "tokens") record.timeline.forEach((entry) => { entry.author_context.tokens += 1000; });
    if (action === "append") record.timeline.push({ type: "comment", id: "tc-reading-appended", author: { kind: "user" },
      body: "An agent appended a new comment while the reader was in old history.", created_at: "2026-10-07T12:01:00Z" });
    await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
  }, action);
  if (action === "append") await page.locator("#comment-tc-reading-appended").waitFor();
}

for (const [label, width, height] of [["phone", 390, 844], ["desktop", 1440, 900]]) {
  it(`keeps the ${label} reader inside a long comment through cache and metadata changes`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReadingPage(page, basePath);
      for (const action of ["tokens", "append", "references", "review", "draft", "age"]) {
        const before = await parkReader(page);
        expect(before).toBeGreaterThan(1000);
        if (["tokens", "append"].includes(action)) await updateRecord(page, action);
        else await page.evaluate(async (action) => {
          const { cache, review, ui, uiStore } = window.__layoutModules;
          const { record, scope } = window.__reading;
          if (action === "references") await cache.writeTasksRecord(scope.deviceId, scope.projectId, {
            tasks: [record.task, { ...record.task, id: "task-referenced", number: 411, title: "Referenced task" }], columns: [],
          });
          if (action === "review") await review.writeReviewRecord(scope, {
            version: 1, snapshots: [{ id: "snapshot-reading", number: 7 }],
          }, 1);
          if (action === "draft") await uiStore.writeUiRecord(ui.uiAddress({
            deviceId: scope.deviceId, entityId: scope.taskId, view: "tracker-task", kind: "draft", sub: scope.projectId,
          }), { body: "Restored draft from another tab" });
          if (action === "age") {
            const now = Date.now();
            Date.now = () => now + 86400000;
            await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
          }
        }, action);
        if (action === "references") await page.locator(`#${READING_ROW} a[href*="task-referenced"]`).waitFor();
        if (action === "review") await page.waitForFunction((id) => document.getElementById(id).textContent.includes("Snapshot 7"), READING_ROW);
        if (action === "draft") await page.waitForFunction(() => document.querySelector("#task-comment").value === "Restored draft from another tab");
        await measure(page, before, `${label}/${action}`);
      }
      await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
    }, { width, height, plugins: [deviceShim] });
  }, 60_000);
}

it("scrolls to a routed task comment once and leaves subsequent paints at the reader's position", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountReadingPage(page, basePath, "tc-reading-18");
    expect(await page.locator(`#${READING_ROW}`).getAttribute("class")).toContain("task-comment-target");
    const routedTop = await page.locator("#task-pane").evaluate((host) => host.scrollTop);
    expect(routedTop).toBeGreaterThan(1000);
    const before = await parkReader(page);
    expect(Math.abs(before - routedTop)).toBeGreaterThan(100);
    await updateRecord(page, "tokens");
    await measure(page, before, "routed/tokens");
    await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
  }, { plugins: [deviceShim] });
}, 60_000);

it("keeps the reader at the bottom when a task comment is appended there", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountReadingPage(page, basePath);
    await page.locator("#task-pane").evaluate((host) => { host.scrollTop = host.scrollHeight; });
    await settle(page);
    await updateRecord(page, "append");
    await settle(page);
    const gap = await page.locator("#task-pane").evaluate((host) => host.scrollHeight - host.clientHeight - host.scrollTop);
    console.log(`bottom/append: ${JSON.stringify({ gap })}`);
    expect(gap).toBeLessThanOrEqual(1);
    await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
  }, { plugins: [deviceShim] });
}, 60_000);
