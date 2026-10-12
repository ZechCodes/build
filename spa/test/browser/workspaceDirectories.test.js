import { afterAll, beforeAll, expect, it } from "vitest";
import { loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); });
afterAll(async () => { await session?.close(); });

async function mountWorkspace(page, basePath, tab) {
  await mountLayout(page, '<div id="dir-rail"></div><main id="root"></main><div id="console">Kept console</div><div id="scrim"><div id="sheet"></div></div>', { basePath });
  await loadBrowserModules(page, {
    app: "src/app.js", view: "src/views/workspaceView.js", cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js", contexts: "src/core/deviceContexts.js", settings: "src/sheets/workspaceSettings.js",
  }, basePath);
  await page.evaluate(async (tab) => {
    const { app, view, cache, feed, contexts } = window.__layoutModules;
    const deviceId = "directory-refresh";
    window.__workspace = { id: "ws", workspace_id: "ws", project_id: "project", name: "Workspace", status: "ready",
      directories: [{ id: "dir-a", source_id: "a", name: "Alpha", is_git: false, status: "ready" }] };
    window.__calls = [];
    const rpc = async (method, params) => {
      window.__calls.push({ method, params });
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "readme.txt", path: "readme.txt", kind: "file" }] };
      if (method === "fs.read") return { mime: "text/plain", content_b64: btoa("Kept file"), size: 9 };
      if (method === "workspace.get") return window.__workspace;
      if (method === "workspace.add_directory" || method === "workspace.remove_directory") return window.__mutation;
      if (method === "workspace.git_init_options") return { source: { is_git: false }, workspace: { is_git: false } };
      if (method === "git.log") return { commits: [] };
      if (method === "git.status") return { staged: [], unstaged: [], untracked: [] };
      return {};
    };
    window.__rpc = rpc;
    const context = contexts.adoptDeviceSession({ deviceId, call: rpc, close() {}, peer() {}, onCarrier() {}, onPush() {} });
    contexts.adoptBridgeSelection(context, { version: "3.14.0" }, { capabilities: {} });
    app.App.devices = [{ id: deviceId, name: "Test device" }];
    app.App.route = { name: "workspace", deviceId, projectId: "project", workspaceId: "ws", sourceId: "a", tab };
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [{ project_id: "project", name: "Project" }]);
    window.__writeWorkspace = async (directories) => {
      window.__workspace = { ...window.__workspace, directories };
      await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, [window.__workspace]);
    };
    await window.__writeWorkspace(window.__workspace.directories);
    await feed.startFeed();
    await view.renderWorkspace();
    window.__console = document.querySelector("#console");
  }, tab);
}

const added = { id: "dir-b", source_id: "b", name: "Beta", is_git: false, status: "pending" };

it("reconciles cached workspace Files roots and loads a pending root when ready, keeping the open file", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "files");
    await page.locator('[data-root="a"] [data-path="readme.txt"]').waitFor();
    await page.locator('[data-root="a"] [data-path="readme.txt"]').dblclick();
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Kept file"));
    await page.evaluate(() => { window.__preview = document.querySelector(".fpdoc").firstElementChild; window.__alpha = document.querySelector('[data-root="a"]'); });
    await page.evaluate((added) => window.__writeWorkspace([...window.__workspace.directories, added]), added);
    await page.locator('[data-root-head="b"]').waitFor({ timeout: 3000 });
    expect(await page.evaluate(() => window.__calls.some((call) => call.method === "fs.tree" && call.params.source_id === "b"))).toBe(false);
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.map((dir) => dir.source_id === "b" ? { ...dir, status: "ready", name: "Renamed Beta" } : dir)));
    await page.locator('[data-root="b"] [data-path="readme.txt"]').waitFor();
    expect(await page.locator('[data-root-head="b"] .fname').textContent()).toBe("Renamed Beta");
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "b")));
    await page.waitForFunction(() => !document.querySelector('[data-root="b"]'));
    expect(await page.evaluate(() => ({ preview: window.__preview === document.querySelector(".fpdoc").firstElementChild,
      root: window.__alpha === document.querySelector('[data-root="a"]'), console: window.__console === document.querySelector("#console"),
      source: window.__layoutModules.app.App.route.sourceId }))).toEqual({ preview: true, root: true, console: true, source: "a" });
  });
}, 60_000);

it("reconciles cached Changes tabs, promotes a pending directory and removes its kept surface", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "changes");
    await page.locator('[data-directory="a"]').waitFor();
    await page.evaluate(() => { window.__alpha = document.querySelector('[data-surface="a"]'); });
    await page.evaluate((added) => window.__writeWorkspace([...window.__workspace.directories, added]), added);
    await page.locator('[data-directory="b"]').waitFor();
    expect(await page.locator('[data-directory="a"]').getAttribute("aria-selected")).toBe("true");
    await page.locator('[data-directory="b"]').click();
    expect(await page.locator('[data-surface="b"] .workspace-gitinit').count()).toBe(0);
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.map((dir) => dir.source_id === "b" ? { ...dir, status: "ready", is_git: true } : dir)));
    await page.locator('[data-surface="b"] .crail-host').waitFor();
    await page.locator('[data-directory="a"]').click();
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "b")));
    await page.waitForFunction(() => !document.querySelector('[data-directory="b"]'));
    expect(await page.locator('[data-surface="b"]').count()).toBe(0);
    expect(await page.evaluate(() => window.__alpha === document.querySelector('[data-surface="a"]'))).toBe(true);
    expect(await page.locator('[data-directory="a"]').getAttribute("aria-selected")).toBe("true");
  });
}, 60_000);

it("publishes settings directory mutations to the workspace list watched by Files", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "files");
    await page.locator('[data-root-head="a"]').waitFor();
    await page.evaluate((added) => {
      window.__mutation = { ...window.__workspace, directories: [...window.__workspace.directories, { ...added, status: "ready" }] };
      window.__layoutModules.settings.openWorkspaceSettings({ id: "ws", name: "Workspace", workspaceKey: "directory-refresh/ws" }, {
        deviceId: "directory-refresh", callRpc: window.__rpc });
    }, added);
    await page.locator("#wsdiradd").selectOption("path");
    await page.locator("#wsdirpath").fill("/tmp/beta");
    await page.locator("#wsdiraddgo").click();
    await page.locator('[data-root-head="b"]').waitFor({ timeout: 3000 });
    await page.evaluate(() => { window.__mutation = window.__workspace; });
    await page.locator('[data-remove-directory="dir-b"]').click();
    await page.waitForFunction(() => !document.querySelector('[data-root-head="b"]'));
  });
}, 60_000);
