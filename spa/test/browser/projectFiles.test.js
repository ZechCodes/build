import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { SHELL_HTML, SHELL_STYLES } from "./filesExplorerSeed.mjs";

it("opens a project source through Files on the project rail", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, SHELL_HTML, { basePath, styles: SHELL_STYLES });
    await loadBrowserModules(page, { app: "src/app.js" }, basePath);
    await loadBrowserModules(page, {
      app: "src/app.js", project: "src/views/projectView.js", contexts: "src/core/deviceContexts.js",
      cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", changes: "src/core/changeEvents.js",
      adapter: "src/core/bridgeApi/v1/index.js", railState: "src/core/projectRailState.js",
    }, basePath);
    await page.evaluate(async () => {
      const { app, contexts, cache, feed, adapter, changes, project } = window.__layoutModules;
      const record = window.__projectRecord = { id: "p", project_id: "p", deviceId: "d", projectKey: "d/p", name: "Build", path: "/build", sources: [{ id: "code", name: "Code" }, { id: "docs", name: "Docs" }] };
      app.App.devices = [{ id: "d", name: "Local", status: "online" }];
      app.App.route = { name: "project", deviceId: "d", projectId: "p", tab: "workspaces" };
      window.__fsCalls = [];
      const call = async (method, params) => {
        window.__fsCalls.push({ method, params });
        if (method === "session.hello") return { api_version: "3.9.0", capabilities: ["fs.projectSources"] };
        if (method === "fs.tree") return { path: params.path, entries: [{ name: "README.md", kind: "file", size: 20 }] };
        if (method === "fs.read") return { path: params.path, mime: "text/markdown", size: 20, content_b64: btoa(`# ${params.source_id} source`), editable: true, encoding: "utf-8", revision: "r1" };
        return {};
      };
      const context = contexts.adoptDeviceSession({ deviceId: "d", call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
      const greeting = { api_version: "3.9.0", capabilities: ["fs.projectSources"] };
      const selected = adapter.create(call, greeting);
      contexts.adoptBridgeSelection(context, { version: "3.9.0" }, selected);
      await changes.greetBridge(call, { deviceId: "d" });
      await cache.writeCached({ deviceId: "d", entityId: "", kind: "projects" }, [record]);
      await feed.startFeed();
      await project.renderProject();
    });
    await page.locator('[data-tab="files"]').click({ timeout: 2000 });
    await page.locator('[data-root="code"] .frow[data-path="README.md"]').dblclick();
    await page.locator('[data-root="docs"] .frow[data-path="README.md"]').dblclick();
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("docs source"));
    expect(await page.evaluate(() => location.hash)).toBe("#/device/d/project/p/files?source=docs&path=README.md");
    expect(await page.evaluate(() => window.__fsCalls.find((call) => call.method === "fs.read" && call.params.source_id === "docs").params)).toMatchObject({ project_id: "p", source_id: "docs", path: "README.md" });
    await page.locator('[data-root-head="code"]').click();
    await captureLayout(page, "project-files.png");
    await page.locator('[data-file-mode="edit"]').click();
    await page.locator(".file-editor").fill("# unsaved project edit");
    await page.evaluate(async () => {
      const { cache, feed } = window.__layoutModules;
      window.__editorBefore = document.querySelector(".file-editor");
      const moved = new Promise((resolve) => {
        const stop = feed.subscribeFeed((snapshot) => {
          if (snapshot.projects.some((record) => record.name === "Renamed Build")) { stop(); resolve(); }
        });
      });
      window.__projectRecord = { ...window.__projectRecord, name: "Renamed Build" };
      await cache.writeCached({ deviceId: "d", entityId: "", kind: "projects" }, [window.__projectRecord]);
      await moved;
    });
    expect(await page.evaluate(() => document.querySelector(".file-editor") === window.__editorBefore)).toBe(true);
    expect(await page.locator(".file-editor").inputValue()).toBe("# unsaved project edit");
    await page.locator('[data-tab="workspaces"]').click();
    await page.locator("[data-confirm-cancel]").click();
    expect(await page.evaluate(() => window.__layoutModules.app.App.route.tab)).toBe("files");
    expect(await page.locator(".file-editor").inputValue()).toBe("# unsaved project edit");
    await page.locator('[data-tab="workspaces"]').click();
    await page.locator("[data-confirm-ok]").click();
    await page.locator('[data-tab="files"]').click();
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("docs source"));
    // A real teardown and explicit project return restore the remembered face
    // and the shared explorer's open-file and collapsed-root UI records.
    await page.evaluate(async () => {
      const { app, project, railState } = window.__layoutModules;
      app.App.viewDispose();
      app.App.route = { name: "trackerTask", deviceId: "d", projectId: "p", taskId: "t" };
      app.App.route = await railState.projectReturnRoute(window.__projectRecord);
      await project.renderProject();
    });
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("docs source"));
    expect(await page.locator('[data-tab="files"]').getAttribute("aria-selected")).toBe("true");
    expect(await page.locator(".ftab").count()).toBe(2);
    expect(await page.locator('[data-root-head="code"]').getAttribute("aria-expanded")).toBe("false");
  }, { width: 1180, height: 840 });
}, 60_000);
