// #410: metadata and cache repainting must leave a reader inside a long
// comment. These checks run production mounts, cache subscribers and styles
// in Chromium; a row which was detached and reinserted also fails identity.
import { expect, it } from "vitest";
import answer from "../../../fixtures/api/v1/tasks.get.json";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const READING_ROW = "comment-tc-reading18";
const SHELL = '<div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div>';
const styles = (basePath) => `@import url("${basePath}src/styles/tasks.css");
  @import url("${basePath}src/styles/surfaces.css");
  html,body{height:100%;margin:0} body{display:block} #tabbody{height:100%}`;

function longRecord() {
  const task = structuredClone(answer.result.task);
  task.watched = true;
  task.read_through = "tc-reading35";
  const paragraph = "The reader is halfway through this detailed comment, following a long explanation of the change and its consequences. ".repeat(9);
  return { task, timeline: Array.from({ length: 36 }, (_, index) => ({
    type: "comment", id: `tc-reading${String(index).padStart(2, "0")}`, author: task.assignee,
    body: index % 3 ? `Short comment ${index}.` : `Comment ${index}: see #411.\n\n${Array.from({ length: 8 }, () => paragraph).join("\n\n")}`,
    created_at: "2026-10-07T12:00:00Z",
    author_context: { tokens: 12345, window: 1000000, compact_at: 200000, at: "2026-10-07T12:00:00Z" },
    attachments: index % 3 ? [] : [{ name: `shot-${index}.png`, path: `/store/shot-${index}.png`, mime: "image/png", size: 1 }],
    ...(index === 18 ? { opinion: { snapshot_id: "snapshot-reading", verdict: "approve" } } : {}),
  })) };
}

async function mountReadingPage(page, basePath, commentId = null, omitTarget = false) {
  await mountLayout(page, SHELL, { basePath, styles: styles(basePath) });
  await loadBrowserModules(page, {
    cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js",
    references: "src/core/referenceIndexFeed.js", review: "src/core/taskReviewCache.js",
    ui: "src/core/localUiState.js", uiStore: "src/core/localUiStore.js",
    local: "src/core/localCache.js", support: "src/core/taskReviewSupport.js",
    reviewFiles: "src/core/taskReviewFiles.js", pages: "src/core/bodyPages.js",
  }, basePath);
  await page.evaluate(async ({ record, commentId, omitTarget }) => {
    const { cache, taskPage, references } = window.__layoutModules;
    const errors = [];
    window.addEventListener("error", (event) => errors.push(event.message));
    window.addEventListener("unhandledrejection", (event) => errors.push(String(event.reason)));
    const fullRecord = structuredClone(record);
    if (omitTarget) record.timeline = record.timeline.filter((entry) => entry.id !== commentId);
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
    const host = document.querySelector("#task-pane");
    const scrollCalls = [];
    const scrollTo = host.scrollTo.bind(host);
    host.scrollTo = (...args) => { scrollCalls.push(args[0]); scrollTo(...args); };
    const mounted = taskPage.mountTaskPage(host, {
      ...scope, projectKey: `${scope.deviceId}/${scope.projectId}`, commentId, feed: () => feed,
      callRpc: (method) => method === "tasks.attachment" ? Promise.resolve(attachment) : new Promise(() => {}),
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
    window.__reading = { record, fullRecord, scope, mounted, stopIndex, errors, scrollCalls };
  }, { record: longRecord(), commentId, omitTarget });
  await page.waitForFunction(() => {
    const images = [...document.querySelectorAll("#task-pane img.thread-attachment-image")];
    return document.querySelectorAll("#task-pane .task-comment").length >= 35 &&
      images.length >= 11 && images.every((image) => image.complete && image.naturalHeight === 180);
  });
  await settle(page);
}

async function settle(page) {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  await page.waitForTimeout(100);
}

async function parkReader(page, fraction = 0.5) {
  await page.evaluate(({ id, fraction }) => {
    const host = document.querySelector("#task-pane");
    const row = document.getElementById(id);
    const body = row.querySelector(".task-comment-body");
    host.scrollTop += body.getBoundingClientRect().top + body.offsetHeight * fraction - host.getBoundingClientRect().top - host.clientHeight / 2;
  }, { id: READING_ROW, fraction });
  await settle(page);
  return page.evaluate((id) => {
    const host = document.querySelector("#task-pane");
    const row = document.getElementById(id);
    const viewport = host.getBoundingClientRect();
    const readingTop = Math.max(viewport.top, host.querySelector(".task-page-head").getBoundingClientRect().bottom);
    const body = row.querySelector(".task-comment-body");
    const content = [...body.querySelectorAll("p, pre, li")].find((element) => {
      const box = element.getBoundingClientRect();
      return box.top < viewport.bottom && box.bottom > readingTop;
    }) || body;
    window.__reading.saved = { row, body, image: row.querySelector("img"), content, contentTop: content.getBoundingClientRect().top };
    window.__reading.scrolls = [];
    window.__reading.contentScrolls = [];
    if (!window.__reading.scrollListener) {
      window.__reading.scrollListener = () => {
        window.__reading.scrolls.push(host.scrollTop);
        window.__reading.contentScrolls.push(window.__reading.saved.content.getBoundingClientRect().top - window.__reading.saved.contentTop);
      };
      host.addEventListener("scroll", window.__reading.scrollListener);
    }
    const topRow = [...host.querySelectorAll(".task-comment")].find((entry) => {
      const box = entry.getBoundingClientRect();
      return box.top <= host.getBoundingClientRect().top + 1 && box.bottom > host.getBoundingClientRect().top + 1;
    });
    window.__reading.saved.topRow = topRow;
    window.__reading.saved.reviewHeight = host.querySelector("[data-task-review]").getBoundingClientRect().height;
    window.__reading.saved.rowTop = row.getBoundingClientRect().top;
    return host.scrollTop;
  }, READING_ROW);
}

async function measure(page, before, label) {
  await settle(page);
  const result = await page.evaluate(({ id, before }) => {
    const host = document.querySelector("#task-pane");
    const row = document.getElementById(id);
    const saved = window.__reading.saved;
    const topRow = [...host.querySelectorAll(".task-comment")].find((entry) => {
      const box = entry.getBoundingClientRect();
      return box.top <= host.getBoundingClientRect().top + 1 && box.bottom > host.getBoundingClientRect().top + 1;
    });
    return { before, after: host.scrollTop, delta: host.scrollTop - before,
      sameRow: saved.row === row, sameBody: saved.body === row.querySelector(".task-comment-body"),
      sameImage: saved.image === row.querySelector("img"), sameTopRow: saved.topRow === topRow,
      rootTopDelta: row.getBoundingClientRect().top - saved.rowTop,
      anchor: saved.content.tagName.toLowerCase(),
      contentTopDelta: saved.content.getBoundingClientRect().top - saved.contentTop,
      sameContent: saved.content.isConnected && row.contains(saved.content),
      reviewHeightDelta: host.querySelector("[data-task-review]").getBoundingClientRect().height - saved.reviewHeight,
      scrolls: window.__reading.scrolls, contentScrolls: window.__reading.contentScrolls };
  }, { id: READING_ROW, before });
  console.log(`${label}: ${JSON.stringify(result)}`);
  expect.soft(Math.abs(result.contentTopDelta), `${label}: visible content moved`).toBeLessThanOrEqual(1);
  expect.soft(result.contentScrolls.every((delta) => Math.abs(delta) <= 1), `${label}: transient content jump`).toBe(true);
  expect.soft(result.sameContent, `${label}: reading content replaced`).toBe(true);
  if (/delayed-(listing|patch|blob|tree|page-first|page-next|failure)$/.test(label)) {
    expect.soft(Math.abs(result.reviewHeightDelta), `${label}: cache arrival must change review height`).toBeGreaterThan(1);
  }
  if (!label.includes("metadata-")) expect.soft(Math.abs(result.rootTopDelta), `${label}: row geometry moved`).toBeLessThanOrEqual(1);
  if (label.endsWith("review-label")) {
    expect.soft(Math.abs(result.reviewHeightDelta), `${label}: review panel footprint changed`).toBeLessThanOrEqual(1);
  }
  expect.soft(result.sameTopRow, `${label}: comment under viewport top changed`).toBe(true);
  expect.soft(result.sameRow, `${label}: reading row replaced`).toBe(true);
  expect.soft(result.sameBody, `${label}: reading body replaced`).toBe(true);
  expect.soft(result.sameImage, `${label}: loaded attachment replaced`).toBe(true);
}

async function updateRecord(page, action) {
  await page.evaluate(async (action) => {
    const { cache } = window.__layoutModules;
    const { record, scope } = window.__reading;
    if (action === "tokens") record.timeline.forEach((entry) => { entry.author_context.tokens += 1000; });
    if (action === "append") record.timeline.push({ type: "comment", id: "tc-reading36", author: record.task.assignee,
      body: "An agent appended a new comment while the reader was in old history.", created_at: "2026-10-07T12:01:00Z" });
    await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
  }, action);
  if (action === "append") await page.locator("#comment-tc-reading36").waitFor();
}

async function updateMetadata(page, action) {
  await page.evaluate(async (action) => {
    const { cache, review, ui, uiStore } = window.__layoutModules;
    const { record, scope } = window.__reading;
    if (action === "references") await cache.writeTasksRecord(scope.deviceId, scope.projectId, {
      tasks: [record.task, { ...record.task, id: "task-referenced", number: 411, title: "Referenced task" }], columns: [],
    });
    if (action.startsWith("review")) {
      const snapshot = (id, number) => ({ id, number, created_at: "2026-10-07T12:00:00Z", author: record.task.assignee, directories: [] });
      const snapshots = [snapshot("snapshot-other", 6)];
      if (action === "review-label") snapshots.push(snapshot("snapshot-reading", 7));
      await review.writeReviewRecord(scope, {
        task_id: scope.taskId, workspace_id: "ws-reading", state: "open", version: action === "review-label" ? 2 : 1,
        snapshots, actions: [], destinations: [], completion: null,
      }, action === "review-label" ? 2 : 1);
    }
    if (action.startsWith("metadata-")) {
      const entry = record.timeline.find((row) => row.id === "tc-reading18");
      entry.opinion = action === "metadata-add" ? { snapshot_id: "snapshot-reading", verdict: "approve" } : null;
      await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
    }
    if (action === "draft") await uiStore.writeUiRecord(ui.uiAddress({
      deviceId: scope.deviceId, entityId: scope.taskId, view: "tracker-task", kind: "draft", sub: scope.projectId,
    }), { body: "Restored draft from another tab" });
    if (action === "age") {
      const now = Date.now();
      Date.now = () => now + 86400000;
      await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
    }
  }, action);
}

for (const [label, width, height] of [["phone", 390, 844], ["desktop", 1440, 900]]) {
  it(`keeps the ${label} reader inside a long comment through cache and metadata changes`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReadingPage(page, basePath);
      for (const action of ["tokens", "append", "references", "review", "review-label", "metadata-remove", "metadata-add", "draft", "age"]) {
        const before = await parkReader(page);
        expect(before).toBeGreaterThan(1000);
        if (["tokens", "append"].includes(action)) await updateRecord(page, action);
        else await updateMetadata(page, action);
        if (action === "references") await page.locator(`#${READING_ROW} a[href*="task-referenced"]`).waitFor();
        if (action === "review-label") await page.waitForFunction((id) => document.getElementById(id).textContent.includes("Snapshot 7"), READING_ROW);
        if (action === "draft") await page.waitForFunction(() => document.querySelector("#task-comment").value === "Restored draft from another tab");
        await measure(page, before, `${label}/${action}`);
      }
      expect(await page.evaluate(() => window.__reading.errors)).toEqual([]);
      await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
    }, { width, height, plugins: [deviceShim] });
  }, 60_000);
}

it.each([false, true])("scrolls to a routed task comment once (late arrival: %s)", async (late) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountReadingPage(page, basePath, "tc-reading18", late);
    if (late) {
      expect(await page.locator("#task-pane").evaluate((host) => host.scrollTop)).toBe(0);
      await page.evaluate(async () => {
        const { cache } = window.__layoutModules;
        const state = window.__reading;
        state.record = state.fullRecord;
        await cache.writeTaskRecord(state.scope.deviceId, state.scope.projectId, state.scope.taskId, state.record);
      });
      await page.locator(`#${READING_ROW}.task-comment-target`).waitFor();
      await settle(page);
    }
    expect(await page.locator(`#${READING_ROW}`).getAttribute("class")).toContain("task-comment-target");
    await page.waitForFunction((id) => {
      const host = document.querySelector("#task-pane");
      const row = document.getElementById(id).getBoundingClientRect();
      const viewport = host.getBoundingClientRect();
      return row.top < viewport.bottom && row.bottom > viewport.top;
    }, READING_ROW);
    expect(await page.evaluate(() => window.__reading.scrollCalls.length)).toBe(1);
    const routedTop = await page.locator("#task-pane").evaluate((host) => host.scrollTop);
    expect(routedTop).toBeGreaterThan(1000);
    for (const action of ["tokens", "references", "review", "review-label", "age", "append"]) {
      const before = await parkReader(page, 0.7);
      expect(Math.abs(before - routedTop)).toBeGreaterThan(100);
      if (["tokens", "append"].includes(action)) await updateRecord(page, action);
      else await updateMetadata(page, action);
      await measure(page, before, `routed/${late}/${action}`);
      expect(await page.locator(`#${READING_ROW}`).getAttribute("class")).toContain("task-comment-target");
      expect(await page.evaluate(() => window.__reading.scrollCalls.length)).toBe(1);
    }
    expect(await page.evaluate(() => window.__reading.errors)).toEqual([]);
    await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
  }, { plugins: [deviceShim] });
}, 60_000);

it("shows unread appended activity without moving history until the reader presses the pill", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountReadingPage(page, basePath);
    const before = await parkReader(page);
    await updateRecord(page, "append");
    await measure(page, before, "unread/append");
    const pill = page.locator(".new-messages-pill");
    await pill.waitFor({ state: "visible" });
    expect(await page.locator(".task-unread-line").count()).toBe(1);
    await pill.click();
    await page.waitForFunction(() => document.querySelector(".task-unread-line").getBoundingClientRect().top < document.querySelector("#task-pane").getBoundingClientRect().bottom);
    await settle(page);
    const after = await page.locator("#task-pane").evaluate((host) => host.scrollTop);
    expect(after).toBeGreaterThan(before + 1000);
    expect(await page.evaluate(() => window.__reading.errors)).toEqual([]);
    await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
  }, { plugins: [deviceShim] });
}, 60_000);

it("keeps history still after scrolling away from a focused composer", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountReadingPage(page, basePath);
    await page.locator("#task-comment").evaluate((field) => field.focus({ preventScroll: true }));
    const before = await parkReader(page);
    await updateMetadata(page, "review");
    await measure(page, before, "focused-history/review");
    expect(await page.locator("#task-comment").evaluate((field) => document.activeElement === field)).toBe(true);
    expect(await page.evaluate(() => window.__reading.errors)).toEqual([]);
    await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
  }, { plugins: [deviceShim] });
}, 60_000);

it("leaves scroll writes to the reader during touch motion", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountReadingPage(page, basePath);
    const before = await parkReader(page);
    await page.evaluate(() => {
      const host = document.querySelector("#task-pane");
      const property = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop");
      window.__reading.scrollWrites = [];
      Object.defineProperty(host, "scrollTop", {
        configurable: true, get: () => property.get.call(host),
        set: (value) => { window.__reading.scrollWrites.push(value); property.set.call(host, value); },
      });
      host.dispatchEvent(new Event("touchstart"));
    });
    await updateRecord(page, "append");
    await measure(page, before, "motion/append");
    expect(await page.evaluate(() => window.__reading.scrollWrites)).toEqual([]);
    await page.evaluate(() => {
      const host = document.querySelector("#task-pane");
      host.dispatchEvent(new Event("touchend"));
      delete host.scrollTop;
      window.__reading.mounted.dispose(); window.__reading.stopIndex();
    });
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


async function mountDelayedReview(page) {
  await page.evaluate(async () => {
    const { support, review } = window.__layoutModules;
    const { scope, record } = window.__reading;
    const directory = { id: "dir-async", source_id: "source-async", name: "Saved source", is_git: true,
      status: "git", path: "/saved/source", source_path: "/source", branch: "build/scroll", head: "2".repeat(40),
      base: { kind: "configured", name: "main", oid: "1".repeat(40) } };
    const snapshot = { id: "snapshot-async", number: 8, created_at: "2026-10-07T12:00:00Z",
      author: record.task.assignee, directories: [directory] };
    window.__reading.reviewIdentity = { ...scope, snapshot, directory };
    await support.rememberReviewSupport(scope.deviceId, { reviews: { get: true, diff: true } });
    await review.writeReviewRecord(scope, { task_id: scope.taskId, workspace_id: "ws-reading", state: "open", version: 3,
      snapshots: [snapshot], actions: [], destinations: [], completion: null }, 3);
  });
  await page.locator(".task-review-changes").waitFor({ state: "attached" });
  await settle(page);
}

async function writeDelayedReview(page, action) {
  await page.evaluate(async (action) => {
    const { local, reviewFiles, pages, uiStore } = window.__layoutModules;
    const { scope, reviewIdentity } = window.__reading;
    const { snapshot, directory } = reviewIdentity;
    const base = [scope.taskId, snapshot.id, directory.id];
    const at = (kind, sub) => ({ deviceId: scope.deviceId, entityId: scope.projectId, kind, sub: JSON.stringify(sub) });
    const fileAt = (kind, path) => reviewFiles.taskReviewFileAddress(reviewIdentity, kind, path);
    const encode = (text) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
    const lines = (count, prefix) => Array.from({ length: count }, (_, index) => `${prefix} ${index}`).join("\n") + "\n";
    if (action === "listing") await local.writeCached(at("task-review-changes", base), {
      files: Array.from({ length: 30 }, (_, index) => ({ path: `file${index}.js`, status: "Modified", additions: 40,
        deletions: 0, content_key: `key-${index}` })), files_truncated: false, diff_key: "base:head",
    });
    if (action === "patch") await local.writeCached(at("task-review-patch", [...base, "file0.js", "old..new"]), {
      content_key: "key-0", patch: "diff --git a/file0.js b/file0.js\n--- a/file0.js\n+++ b/file0.js\n@@ -0,0 +1,40 @@\n" +
        Array.from({ length: 40 }, (_, index) => `+saved line ${index + 1}`).join("\n") + "\n", truncated: false,
    });
    if (action === "blob") {
      const text = lines(24, "Saved file line");
      await local.writeCached(fileAt("task-review-file", "notes.txt"), { file: {
        path: "notes.txt", mime: "text/plain", size: text.length, content_b64: encode(text), truncated: false, editable: false,
      } });
    }
    if (action === "tree") await local.writeCached(fileAt("task-review-tree", ""), {
      path: "", entries: [{ name: "notes.txt", kind: "file" }, { name: "paged.txt", kind: "file" },
        ...Array.from({ length: 40 }, (_, index) => ({ name: `saved-${index}.txt`, kind: "file" }))],
    });
    if (action === "paged-head") await local.writeCached(fileAt("task-review-file", "paged.txt"), { file: {
      path: "paged.txt", mime: "text/plain", size: 2000000, paged: true, of: "paged-reading", truncated: false, editable: false,
    } });
    if (action.startsWith("page-")) {
      const first = lines(100, "Paged saved line");
      const second = lines(20, "More paged saved line");
      const offset = action === "page-first" ? 0 : first.length;
      const body = action === "page-first" ? first : second;
      await pages.writeBodyPage(fileAt("task-review-file", "paged.txt"), { of: "paged-reading", offset,
        end: offset + body.length, total: first.length + second.length, body: encode(body) });
    }
    if (action === "failure") await local.writeCached(fileAt("task-review-file-error", "paged.txt"), {
      message: "Delayed saved file read failed. ".repeat(30),
    });
    if (action === "tabs-paged" || action === "tabs-restored") await uiStore.writeUiRecord(fileAt("ui-task-review-files", "tabs"), {
      tabs: ["notes.txt", "paged.txt"], active: action === "tabs-paged" ? "paged.txt" : "notes.txt",
    });
  }, action);
}

for (const [label, width, height] of [["phone", 390, 844], ["desktop", 1440, 900]]) {
  it(`keeps visible ${label} task text still through delayed saved-review child cache paints`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReadingPage(page, basePath);
      await mountDelayedReview(page);
      await page.locator("#task-comment").evaluate((field) => field.focus({ preventScroll: true }));
      let before = await parkReader(page);
      await writeDelayedReview(page, "listing");
      await page.waitForFunction(() => document.querySelectorAll("[data-review-path]").length === 30);
      await measure(page, before, `${label}/delayed-listing`);
      await page.locator('[data-review-path="file0.js"] [data-review-expand]').evaluate((button) => button.click());
      await settle(page);
      before = await parkReader(page);
      await writeDelayedReview(page, "patch");
      await page.locator('[data-review-path="file0.js"] tr[data-new-line="40"]').waitFor({ state: "attached" });
      await measure(page, before, `${label}/delayed-patch`);
      await page.evaluate(async () => {
        const { ui, uiStore } = window.__layoutModules;
        const { scope } = window.__reading;
        await uiStore.writeUiRecord(ui.uiAddress({ deviceId: scope.deviceId, entityId: scope.projectId,
          view: "task-review", kind: "selection", sub: scope.taskId }), {
          snapshotId: "snapshot-async", directoryId: "dir-async", view: "files", path: "notes.txt",
        });
      });
      await page.locator(".trf-preview .throbber").waitFor({ state: "attached" });
      await settle(page);
      before = await parkReader(page);
      await writeDelayedReview(page, "blob");
      await page.locator('.trf-preview tr[data-new-line="24"]').waitFor({ state: "attached" });
      await measure(page, before, `${label}/delayed-blob`);
      before = await parkReader(page);
      await writeDelayedReview(page, "tree");
      await page.waitForFunction(() => document.querySelectorAll(".trf-tree .frow").length === 42);
      await measure(page, before, `${label}/delayed-tree`);
      before = await parkReader(page);
      await writeDelayedReview(page, "tabs-paged");
      await page.locator('.ftab.active [data-tab-path="paged.txt"]').waitFor({ state: "attached" });
      await settle(page);
      await measure(page, before, `${label}/delayed-tabs-placeholder`);
      before = await parkReader(page);
      await writeDelayedReview(page, "paged-head");
      await page.locator(".trf-paged .fppages").waitFor({ state: "attached" });
      await measure(page, before, `${label}/delayed-paged-head`);
      for (const action of ["page-first", "page-next", "failure", "tabs-restored"]) {
        before = await parkReader(page);
        await writeDelayedReview(page, action);
        if (action === "page-first") await page.locator('.trf-paged tr[data-new-line="100"]').waitFor({ state: "attached" });
        if (action === "page-next") await page.locator('.trf-paged tr[data-new-line="120"]').waitFor({ state: "attached" });
        if (action === "failure") await page.waitForFunction(() => document.querySelector(".trf-status").textContent.includes("Delayed saved file read failed"));
        if (action === "tabs-restored") await page.locator('.trf-preview tr[data-new-line="24"]').waitFor({ state: "attached" });
        await measure(page, before, `${label}/delayed-${action}`);
      }
      // A deliberate review-anchor press must still navigate after a wrapped
      // cache paint; preserving history must never undo that explicit reveal.
      before = await parkReader(page);
      await page.evaluate(async () => {
        const { cache } = window.__layoutModules;
        const { record, scope } = window.__reading;
        record.timeline.find((row) => row.id === "tc-reading18").anchor = {
          snapshot_id: "snapshot-async", directory_id: "dir-async", path: "file0.js", side: "new", line: 40,
        };
        await cache.writeTaskRecord(scope.deviceId, scope.projectId, scope.taskId, record);
      });
      await page.locator(`#${READING_ROW} [data-review-anchor]`).evaluate((button) => button.click());
      await page.locator('tr.task-review-anchor[data-new-line="40"]').waitFor({ state: "attached" });
      await page.waitForFunction((before) => document.querySelector("#task-pane").scrollTop < before - 1000, before);
      expect(await page.locator('[data-review-path="file0.js"] [data-review-expand]').getAttribute("aria-expanded")).toBe("true");
      expect(await page.evaluate(() => window.__reading.errors)).toEqual([]);
      await page.evaluate(() => { window.__reading.mounted.dispose(); window.__reading.stopIndex(); });
    }, { width, height, plugins: [deviceShim] });
  }, 90_000);
}
