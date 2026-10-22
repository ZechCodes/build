import { readFile } from "node:fs/promises";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";
import { TIMING_TABLE } from "./markdownPolishFixture.mjs";
import { SHELL_HTML, SHELL_STYLES } from "./filesExplorerSeed.mjs";

const answer = JSON.parse(await readFile(new URL("../../../fixtures/api/v1/tasks.get.json", import.meta.url), "utf8"));
export const REPORT = `# Markdown review\n\nA readable report with **clear priorities**, \`npm test\`, and <kbd>Enter</kbd>.\n\n## Timing at a glance\n\n${TIMING_TABLE}\n\nThe measurements stay together.[^timing]\n\n### Review checklist\n\n- [x] Borderless tables\n- [x] Phone layout\n- [ ] Final review\n\n[^timing]: All values are milliseconds.`;
export const PLAN = `# Markdown rendering plan\n\nShared typography makes chat, tasks, and documents easier to read.\n\n## Goals\n\nKeep prose readable and measurements together.\n\n### Verification\n\n${TIMING_TABLE}\n\n#### Keyboard access\n\nOnly overflowing tables need a keyboard stop.\n\n##### Privacy\n\nExternal images load after an explicit choice.\n\n###### Evidence\n\nCapture both themes and real application surfaces.`;

async function configure(page, basePath, theme) {
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  await page.addScriptTag({ type: "module", content: `
    import { installMarkdownTableFocus } from "${basePath}src/core/markdownTableFocus.js";
    installMarkdownTableFocus();
    window.__markdownSurfaceReady = true;
  ` });
  await page.waitForFunction(() => window.__markdownSurfaceReady);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

export async function mountAgentMarkdown(page, basePath, theme) {
  await mountLayout(page, '<main id="conversation"></main>', {
    basePath, styles: "html,body{height:auto;overflow:auto}body{padding:12px}#conversation{max-width:760px;margin:auto}",
  });
  await loadBrowserModules(page, { thread: "src/core/thread.js" }, basePath);
  await page.evaluate((body) => {
    const { threadHtml, createThreadState } = window.__layoutModules.thread;
    const threadState = createThreadState();
    threadState.openArrival("markdown-report", true);
    document.querySelector("#conversation").innerHTML = threadHtml({
      id: "markdown-review-thread", items: [{ type: "message", data: {
        id: "markdown-report", sequence: 1, role: "user", body,
        from_agent: { id: "reviewer", owner: { kind: "project", id: "proj-1", name: "Build" }, topic: "Markdown review" },
      } }],
    }, { place: { deviceId: "capture-device", projectId: "proj-1" }, threadState });
  }, REPORT);
  await configure(page, basePath, theme);
}

export async function mountTaskMarkdown(page, basePath, theme) {
  await mountLayout(page, '<div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div>', {
    basePath, styles: `@import url("${basePath}src/styles/tasks.css"); @import url("${basePath}src/styles/surfaces.css");
      html,body{height:100%;margin:0}body{display:block}#tabbody{height:100%}`,
  });
  await loadBrowserModules(page, { cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js" }, basePath);
  const task = structuredClone(answer.result.task);
  task.number = 695;
  task.title = "Superb markdown in chat and tasks";
  task.body = "Review shared typography, table wrapping, and keyboard access.";
  await page.evaluate(async ({ task, body }) => {
    const { cache, taskPage } = window.__layoutModules;
    const record = { task, timeline: [{
      type: "comment", id: "markdown-comment", body, author: task.assignee,
      created_at: "2026-10-20T15:00:00Z", attachments: [],
    }] };
    await cache.writeTasksRecord("capture-device", "proj-1", { tasks: [task], columns: [] });
    await cache.writeTaskRecord("capture-device", "proj-1", task.id, record);
    window.__markdownPage = taskPage.mountTaskPage(document.querySelector("#task-pane"), {
      deviceId: "capture-device", projectId: "proj-1", projectKey: "capture-device/proj-1", taskId: task.id,
      feed: () => ({ projects: [], workspaces: [], items: [] }), callRpc: () => new Promise(() => {}),
      catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
    });
  }, { task, body: REPORT });
  await page.locator(".task-comment-body .mdtable").waitFor();
  await configure(page, basePath, theme);
}

export async function mountPlanMarkdown(page, basePath, theme) {
  await mountLayout(page, SHELL_HTML, { basePath, styles: SHELL_STYLES });
  await loadBrowserModules(page, { files: "src/views/files.js", scope: "src/core/cacheScope.js" }, basePath);
  await page.evaluate((source) => {
    document.querySelector("#toolbar").textContent = "Build / Markdown / Files";
    const { files, scope } = window.__layoutModules;
    const bytes = new TextEncoder().encode(source);
    const file = { path: "Plan.md", mime: "text/markdown", size: bytes.length, truncated: false,
      editable: true, encoding: "utf-8", revision: "r1", content_b64: btoa(String.fromCharCode(...bytes)) };
    window.__markdownFiles = files.renderFilesTab(document.querySelector("#tabbody"), {
      scope: { run_id: "markdown-capture" }, cacheScope: scope.scopeFor("capture-device"),
      openAt: { path: "Plan.md" },
      callRpc: async (method) => method === "fs.tree"
        ? { path: "", entries: [{ name: "Plan.md", kind: "file", size: bytes.length }] } : file,
    });
  }, PLAN);
  await page.locator(".plan h6").waitFor();
  await configure(page, basePath, theme);
}
