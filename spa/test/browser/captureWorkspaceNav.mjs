// Capture the workspace view's navigation for #174, in a real Chromium against
// the production app shell: the toolbar's `project / workspace` picker, the
// rail (Changes, Files, Tasks, Settings), the Files tree with a root per
// directory, and the Changes tab row standing on the directory without git —
// and a failed workspace's Retry on that surface, where the bar has none.
// A workspace with two directories, one of them not git, on one scripted
// machine; everything else is the app's own modules and sheets.
// Run from spa/: node test/browser/captureWorkspaceNav.mjs [output directory]
import { mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const output = resolve(process.argv[2] || "/tmp/workspace-nav");
await mkdir(output, { recursive: true });
const indexHtml = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const bodyHtml = indexHtml.match(/<body>([\s\S]*)<\/body>/)[1];

/** Runs in the page: the machine's records, a session answering it, and the
 *  app standing on the workspace. */
async function standOnWorkspace({ hash, theme, status }) {
  document.documentElement.dataset.theme = theme;
  // The inbox folded away, as a reader working in a workspace keeps it: the
  // capture is of the workspace, and the inbox is not mounted here.
  document.body.classList.add("inbox-collapsed");
  const { app, cache, feed, merge, contexts, toolbar, events, router } = window.__layoutModules;
  const project = { project_id: "p-1", name: "Build", path: "/home/zech/Projects/build" };
  const workspace = {
    id: "ws-1", workspace_id: "ws-1", project_id: "p-1", name: "Workspace nav", status,
    root: "/home/zech/.build/workspaces/ws-1", entity_id: "run-1",
    directories: [
      { source_id: "build", name: "Build", is_git: true },
      { source_id: "design", name: "Design assets", is_git: false },
    ],
  };
  const trees = {
    build: {
      "": [
        { name: "bridge", kind: "dir" }, { name: "spa", kind: "dir" },
        { name: "ARCHITECTURE.md", kind: "file", size: 36525 }, { name: "README.md", kind: "file", size: 2802 },
      ],
      spa: [{ name: "src", kind: "dir" }, { name: "index.html", kind: "file", size: 3120 }, { name: "package.json", kind: "file", size: 1320 }],
    },
    design: {
      "": [
        { name: "landing", kind: "dir" }, { name: "logo.svg", kind: "file", size: 1804 },
        { name: "palette.md", kind: "file", size: 612 }, { name: "README.md", kind: "file", size: 404 },
      ],
    },
  };
  const encode = (text) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
  const text = (path) => `# ${path}\n\nEverything an agent working in this repository needs to follow.\n\n## Read first\n\nRead ARCHITECTURE.md before changing spa/ or bridge/.\n`;
  const commit = (hash, subject, minutes) => ({ hash, short: hash.slice(0, 7), subject, author: "Zech Zimmerman", time: Math.floor(Date.now() / 1000) - minutes * 60 });
  const answers = {
    "fs.tree": (params) => ({ path: params.path, entries: trees[params.source_id]?.[params.path] || [] }),
    "fs.read": (params) => ({ path: params.path, mime: "text/markdown", size: 180, truncated: false, editable: true, encoding: "utf-8", revision: "r1", content_b64: encode(text(params.path)) }),
    "git.status": () => ({ branch: "build/workspace-nav-174", head: "ea34c2cd", repo_state: "clean", upstream: "origin/main", ahead: 3, behind: 0, files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } }),
    "git.log": () => ({ branch: "build/workspace-nav-174", commits: [
      commit("8d5bfd59aaaaaaaa", "files: a workspace's Files is one tree, one collapsible root per directory", 12),
      commit("59514cceaaaaaaaa", "rail: the workspace's navigation — Changes, Files, Tasks, Settings", 40),
      commit("861af6a9aaaaaaaa", "toolbar: a workspace's bar is one picker reading \"project / workspace\"", 65),
    ], more: false }),
    "git.refs": () => ({ current: { kind: "branch", name: "build/workspace-nav-174", full_ref: "refs/heads/build/workspace-nav-174" }, refs: [] }),
    "workspace.git_init_options": (params) => ({
      workspace_id: "ws-1", source_id: params.source_id,
      workspace: { path: "/home/zech/.build/workspaces/ws-1/design", is_git: false, available: true },
      source: { path: "/home/zech/Design assets", is_git: false, available: true },
    }),
  };
  const call = async (method, params = {}) => (answers[method] ? answers[method](params) : {});
  await events.greetBridge(async () => ({
    api_version: "2.0.0", push_events: true,
    capabilities: ["changes.subscriptions", "requests.priority", "errors.codes", "diffs.perFile", "tasks.context",
      "tasks.attachments", "tasks.watching", "conversations.settings"],
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"], items: "bodies" },
  }), { deviceId: "dev-1" });

  // Past the connection gate, the way a paired browser stands.
  app.App.gated = false;
  app.App.devices = [{ id: "dev-1", name: "workshop", status: "online" }];
  await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
  const view = merge.liveFeedSnapshot({ items: [], runs: [] }, { projects: [project] }, { workspaces: [workspace] }, "dev-1");
  await cache.writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, view);
  await cache.writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, view.projects);
  await cache.writeCached({ deviceId: "dev-1", entityId: "", kind: "workspaces" }, view.workspaces);
  contexts.adoptDeviceSession({ deviceId: "dev-1", call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
  await feed.startFeed();
  await toolbar.initToolbar();
  app.initRouter();
  location.hash = hash;
  app.App.route = router.routeFromHash(hash);
  app.render();
}

const MODULES = {
  app: "src/app.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", merge: "src/core/feedMerge.js",
  contexts: "src/core/deviceContexts.js", toolbar: "src/core/toolbar.js", events: "src/core/changeEvents.js",
  router: "src/core/router.js",
};

/** The app's own page head — its charset (the sheets' `content:` glyphs are
 *  UTF-8) and its viewport — over the app's body. */
async function mountApp(page, basePath) {
  // Served as a page of its own: a document written over the harness's first
  // one keeps that one's encoding, and the sheets would read as Latin-1.
  const url = new URL(`${basePath}workspace-nav-capture.html`, page.url()).href;
  await page.route(url, (route) => route.fulfill({
    contentType: "text/html; charset=utf-8",
    body: `<!doctype html><html><head><meta charset="utf-8">
      <meta name="viewport" content="width=device-width, initial-scale=1">
      <link rel="stylesheet" href="${basePath}src/styles.css">
      <link rel="stylesheet" href="${basePath}src/styles/shell.css"></head><body>${bodyHtml}</body></html>`,
  }));
  await page.goto(url, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
}

async function open(page, basePath, hash, theme, status = "ready") {
  await mountApp(page, basePath);
  // The app first, the way main.js imports it: its module graph has cycles
  // that only settle in that order.
  await loadBrowserModules(page, { app: MODULES.app }, basePath);
  await page.evaluate(() => { window.__app = window.__layoutModules.app; });
  await loadBrowserModules(page, MODULES, basePath);
  await page.evaluate(standOnWorkspace, { hash, theme, status });
}

const FILES = "#/device/dev-1/project/p-1/workspace/ws-1/directory/design/files?path=palette.md";
const CHANGES = "#/device/dev-1/project/p-1/workspace/ws-1/directory/design/changes";

async function captureWidth(page, basePath, { name, theme, phone = false }) {
  page.on("console", (message) => { if (process.env.DEBUG_CAPTURE) console.error("page:", message.text()); });
  page.on("pageerror", (error) => console.error("pageerror:", error.message));
  await open(page, basePath, FILES, theme);
  await page.waitForSelector('.froot[data-root="design"] .frow[data-path="palette.md"]');
  await page.waitForSelector(".fppath", { state: "attached" });
  if (phone) {
    await page.screenshot({ path: `${output}/${name}-files-closed-${theme}.png` });
    await page.locator("[data-pane-handle]").tap();
    await page.waitForTimeout(400);
  } else {
    await page.locator('.froot[data-root="build"] .frow[data-path="spa"]').click();
    await page.waitForSelector('.frow[data-path="spa/src"]');
  }
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${output}/${name}-files-${theme}.png` });
  // The rail's Changes, as a reader would press it, then the directory's tab.
  await page.locator("#dir-rail [data-tab=changes]").click();
  await page.locator('.workspace-dirtab[data-directory="build"]').click();
  await page.waitForSelector(".gitpane .crail-host");
  await page.waitForTimeout(1200);
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${output}/${name}-changes-git-${theme}.png` });
  await page.locator('.workspace-dirtab[data-directory="design"]').click();
  await page.waitForSelector(".workspace-gitinit [data-init-git]");
  await page.mouse.move(5, 5);
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${output}/${name}-changes-nongit-${theme}.png` });
}

/** A workspace whose setup failed: Retry on Changes, under the sentence on the
 *  directory without git and at the foot of the commit rail on the one with it. */
async function captureFailed(page, basePath, { name, theme, phone = false }) {
  await open(page, basePath, CHANGES, theme, "failed");
  await page.waitForSelector(".workspace-gitinit [data-workspace-action]");
  await page.mouse.move(5, 5);
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${output}/${name}-failed-nongit-${theme}.png` });
  await page.locator('.workspace-dirtab[data-directory="build"]').click();
  await page.waitForSelector('[data-surface="build"] .crail-host [data-workspace-action]', { state: "attached" });
  await page.waitForTimeout(1200);
  // On a phone the commit rail is a drawer, and the offers are in it.
  if (phone) {
    await page.locator('[data-surface="build"] [data-pane-handle]').tap();
    await page.waitForTimeout(400);
  } else await page.mouse.move(5, 5);
  await page.screenshot({ path: `${output}/${name}-failed-git-${theme}.png` });
}

for (const theme of ["dark"]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await captureWidth(page, basePath, { name: "desktop", theme });
    await captureFailed(page, basePath, { name: "desktop", theme });
    const phone = await page.context().browser().newContext({
      viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true,
    });
    const tap = await phone.newPage();
    await tap.goto(page.url());
    await captureWidth(tap, basePath, { name: "phone", theme, phone: true });
    await captureFailed(tap, basePath, { name: "phone", theme, phone: true });
    await phone.close();
  }, { width: 1600, height: 900 });
}
console.log(output);
