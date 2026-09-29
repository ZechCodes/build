// Capture the review images for #116: a task whose body and comment carry
// screenshots and a recording, drawn by the production renderer, wired by the
// production attachment wiring and cache, and opened in the one lightbox.
// Run from spa/: node test/browser/captureAttachmentLightbox.mjs [recording.webm]
import { mkdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const design = (path) => fileURLToPath(new URL(`../../../design/${path}`, import.meta.url));
const output = design("task-attachments/");
await mkdir(output, { recursive: true });

const recordingPath = process.argv[2];
const files = {
  "/store/aa-board-desktop.png": { name: "board-desktop.png", mime: "image/png", from: design("tasks-dashboard/tasks111-desktop-dark.png") },
  "/store/bb-board-mobile.png": { name: "board-mobile.png", mime: "image/png", from: design("tasks-dashboard/tasks111-mobile-dark.png") },
  "/store/cc-comment-box.png": { name: "comment-box.png", mime: "image/png", from: design("tasks-compose/compose-1440-dark.png") },
  "/store/dd-vitest.log": { name: "vitest.log", mime: "text/plain", text: "Test Files 388 passed (388)\n" },
};
if (recordingPath) files["/store/ee-drag.webm"] = { name: "drag.webm", mime: "video/webm", from: recordingPath };

const bytes = {};
for (const [path, file] of Object.entries(files)) {
  const content = file.from ? await readFile(file.from) : Buffer.from(file.text);
  bytes[path] = { mime: file.mime, size: content.length, content_b64: content.toString("base64") };
}
const descriptor = (path) => ({ path, name: files[path].name, mime: files[path].mime, size: bytes[path].size });

const answer = {
  task: {
    id: "task-116", project_id: "review-project", number: 116,
    title: "Agents attach their own screenshots and videos to task comments",
    body: "Reviewers' evidence is only listed as `/tmp/*.png` paths. The files below were attached by an agent from its own paths.",
    state: "open", status: "in_review", labels: ["frontend", "bridge"], priority: "high", assignee: null,
    links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
    created_by: { kind: "user" }, created_at: "2026-09-23T21:26:30Z", updated_at: "2026-09-23T21:50:00Z",
    attachments: ["/store/aa-board-desktop.png", "/store/bb-board-mobile.png", "/store/dd-vitest.log"].map(descriptor),
  },
  timeline: [
    { type: "event", id: "te-1", kind: "created", at: "2026-09-23T21:26:30Z", actor: { kind: "user" }, payload: {} },
    {
      type: "comment", id: "tc-1", task_id: "task-116", author: { kind: "user" },
      body: "Before and after, plus the drag recording.", refs: [], created_at: "2026-09-23T21:40:00Z",
      attachments: ["/store/cc-comment-box.png", ...(recordingPath ? ["/store/ee-drag.webm"] : [])].map(descriptor),
    },
  ],
};

const mountTask = async (page, basePath) => {
  await mountLayout(page, '<main class="task-surface" id="task-layout"></main>', {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css");
      body{display:block;margin:0;width:100vw;height:100vh}
      #task-layout{height:100vh;width:100vw;max-width:none;overflow:auto}`,
  });
  await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
  await loadBrowserModules(page, {
    taskRender: "src/core/trackerTaskRender.js",
    timeline: "src/core/trackerTimeline.js",
    thread: "src/core/thread.js",
    bodies: "src/core/taskAttachmentBodies.js",
  }, basePath);
  await page.evaluate(({ answer, bytes }) => {
    const host = document.querySelector("#task-layout");
    const { taskRender, timeline, thread, bodies } = window.__layoutModules;
    host.innerHTML = taskRender.taskPageHtml(answer.task, {
      rows: timeline.timelineRows(answer.timeline), unreadFrom: null,
      columns: [{ id: "in_review", name: "In review" }],
      agentLabels: {}, agentProviders: {}, projectName: "Build",
      links: [], watch: null, draft: "", labelsDraft: "", busy: false,
      sending: false, attachable: false, hasFiles: false,
    });
    // The page's own loader: the bytes go through the cache, read back in
    // `tasks.attachment` pieces from this stand-in bridge.
    const loader = bodies.createTaskAttachmentBodies({
      deviceId: "review-device",
      taskId: answer.task.id,
      call: async (_method, params) => ({ path: params.path, offset: 0, ...bytes[params.path] }),
    });
    thread.wireThreadAttachments(host.querySelector(".task-page-main"), loader.load, thread.createThreadState());
  }, { answer, bytes });
  await page.waitForFunction(() =>
    [...document.querySelectorAll("img.thread-attachment-image")].every((image) => image.complete && image.naturalWidth > 0));
};

const openLightbox = async (page, selector) => {
  await page.locator(selector).first().click();
  await page.waitForFunction(() => {
    const media = document.querySelector(".thread-lightbox-stage img, .thread-lightbox-stage video");
    if (!media) return false;
    return media.tagName === "VIDEO" ? media.readyState >= 3 : media.complete && media.naturalWidth > 0;
  });
  // The dialog opens with a height reveal; wait for it to settle.
  await page.waitForTimeout(1200);
};

for (const [label, viewport] of [["desktop", { width: 1440, height: 900 }], ["mobile", { width: 390, height: 844 }]]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountTask(page, basePath);
    await page.screenshot({ path: `${output}task-thumbnails-${label}.png` });
    await openLightbox(page, '.task-page-attachments button.thread-attachment-preview[data-attachment-kind="image"]');
    await page.screenshot({ path: `${output}lightbox-image-${label}.png` });
    if (recordingPath) {
      await page.keyboard.press("Escape");
      await page.waitForFunction(() => !document.querySelector(".thread-lightbox"));
      await openLightbox(page, '.task-comment-attachments button.thread-attachment-preview[data-attachment-kind="video"]');
      await page.screenshot({ path: `${output}lightbox-video-${label}.png` });
    }
  }, viewport);
}
console.log(`wrote ${output}`);
