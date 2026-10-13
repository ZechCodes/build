import { afterAll, beforeAll, expect, it } from "vitest";
import { loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); });
afterAll(async () => { await session?.close(); });

async function mountWorkspace(page, basePath, tab, directories = null) {
  await mountLayout(page, '<div id="dir-rail"></div><main id="root"></main><div id="console">Kept console</div><div id="scrim"><div id="sheet"></div></div>', { basePath });
  await loadBrowserModules(page, {
    app: "src/app.js", view: "src/views/workspaceView.js", cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js", contexts: "src/core/deviceContexts.js", settings: "src/sheets/workspaceSettings.js",
  }, basePath);
  await page.evaluate(async ({ tab, directories }) => {
    const { app, view, cache, feed, contexts } = window.__layoutModules;
    const deviceId = "directory-refresh";
    window.__workspace = { id: "ws", workspace_id: "ws", project_id: "project", name: "Workspace", status: "ready", entity_id: "workspace-run",
      directories: directories || [{ id: "dir-a", source_id: "a", name: "Alpha", is_git: false, status: "ready" }] };
    window.__calls = [];
    window.__heldReads = [];
    const rpc = async (method, params) => {
      window.__calls.push({ method, params });
      if (method === "fs.tree") return { path: params.path, entries: [{ name: "readme.txt", path: "readme.txt", kind: "file" }] };
      if (method === "fs.read" && window.__holdFileReads) return new Promise((resolve) => window.__heldReads.push({ params, resolve }));
      if (method === "fs.read") return { mime: "text/plain", content_b64: btoa("Kept file"), size: 9, editable: Boolean(window.__fileEditable), revision: "file-v1" };
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
  }, { tab, directories });
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

it("shows the no-git offer when a new directory becomes ready and selects a survivor when it is removed", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "changes");
    await page.locator('[data-directory="a"]').waitFor();
    await page.evaluate((added) => window.__writeWorkspace([...window.__workspace.directories, added]), added);
    await page.locator('[data-directory="b"]').click();
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.map((dir) => dir.source_id === "b" ? { ...dir, status: "ready", name: "Renamed Beta" } : dir)));
    await page.locator('[data-surface="b"] [data-init-git]').waitFor();
    expect(await page.locator('[data-surface="b"] .workspace-gitinit-say').textContent()).toBe("Renamed Beta has no Git repository.");
    await page.locator('[data-surface="b"] [data-init-git]').click();
    await page.locator(".modal-workspace-init").waitFor();
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "b")));
    await page.waitForFunction(() => document.querySelector('[data-directory="a"]')?.getAttribute("aria-selected") === "true");
    expect(await page.locator('[data-surface="b"]').count()).toBe(0);
    expect(await page.evaluate(() => window.__layoutModules.app.App.route.sourceId)).toBe("a");
    expect(await page.locator(".modal-workspace-init").count()).toBe(0);
    await page.evaluate(() => window.__writeWorkspace([]));
    await page.waitForFunction(() => !document.querySelector('[data-surface]'));
    await page.evaluate((added) => window.__writeWorkspace([{ ...added, status: "ready" }]), added);
    await page.locator('[data-surface="b"] [data-init-git]').waitFor();
    expect(await page.locator('[data-directory="b"]').getAttribute("aria-selected")).toBe("true");
  });
}, 60_000);


for (const tab of ["files", "changes"]) {
  it(`mounts a directory added to an initially empty workspace on ${tab}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountWorkspace(page, basePath, tab, []);
      await page.evaluate((added) => window.__writeWorkspace([{ ...added, status: "ready" }]), added);
      await page.locator(tab === "files" ? '[data-root-head="b"]' : '[data-directory="b"]').waitFor({ timeout: 3000 });
    });
  }, 60_000);
}

it("keeps an open surviving file subscribed when removing the first Git directory changes its cache scope", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "files", [
      { id: "dir-a", source_id: "a", name: "Alpha", is_git: true, status: "ready" },
      { ...added, is_git: true, status: "ready" },
    ]);
    await page.locator('[data-root="b"] [data-path="readme.txt"]').dblclick();
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Kept file"));
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "a")));
    await page.waitForFunction(() => !document.querySelector('[data-root="a"]'));
    await page.evaluate(() => window.__layoutModules.cache.writeCached({ deviceId: "directory-refresh", entityId: "workspace-run", kind: "file", sub: "readme.txt" },
      { file: { mime: "text/plain", content_b64: btoa("Updated file"), size: 12 } }));
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Updated file"), null, { timeout: 3000 });
    expect(await page.evaluate(() => window.__layoutModules.app.App.route.sourceId)).toBe("b");
  });
}, 60_000);

for (const warmRunCache of [false, true]) {
  it(`rebinds a kept Changes directory when it becomes the workspace's first Git directory (${warmRunCache ? "warm" : "cold"} run cache)`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountWorkspace(page, basePath, "changes", [
        { id: "dir-a", source_id: "a", name: "Alpha", is_git: true, status: "ready" },
        { ...added, is_git: true, status: "ready" },
      ]);
      // Let Alpha's startup writes settle before choosing the cache path. A
      // populated run cache makes a new git.log RPC unnecessary on rebind.
      await page.waitForFunction(async () => {
        const { cache } = window.__layoutModules;
        const address = { deviceId: "directory-refresh", entityId: "workspace-run" };
        const [status, log, unpushed] = await Promise.all(["status", "log", "unpushed"].map((kind) => cache.readCached({ ...address, kind })));
        return Boolean(status && log && unpushed && !status.value.stale && !log.value.stale);
      });
      await page.locator('[data-directory="b"]').click();
      await page.locator('[data-surface="b"] .crail-host').waitFor();
      await page.evaluate(async (warmRunCache) => {
        const { cache } = window.__layoutModules;
        const address = { deviceId: "directory-refresh", entityId: "workspace-run" };
        if (!warmRunCache) await cache.deleteCached(["status", "log"].map((kind) => ({ ...address, kind })));
        window.__beta = document.querySelector('[data-surface="b"]');
      }, warmRunCache);
      await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "a")));
      await page.locator('[data-directory="a"]').waitFor({ state: "detached" });
      // Only the deliberately emptied cache guarantees a pull. Let that miss
      // reach the reader before publishing, so this case stays cold on startup.
      if (!warmRunCache) await page.waitForFunction(() => window.__calls.some((call) => call.method === "git.log" && call.params.source_id === "b" && call.params.entity_id === "workspace-run"), null, { timeout: 3000 });
      // Observe the new cache subscription, rather than a conditional startup
      // RPC. Publish a second log after the first paint to exercise live writes.
      for (const hash of ["b".repeat(40), "c".repeat(40)]) {
        await page.evaluate(async (hash) => {
          const { cache } = window.__layoutModules;
          const address = { deviceId: "directory-refresh", entityId: "workspace-run" };
          await cache.writeCached({ ...address, kind: "status" }, { staged: [], unstaged: [], untracked: [] });
          await cache.writeCached({ ...address, kind: "log" }, { commits: [{ hash, short: hash.slice(0, 7), subject: "Rebound history", author: "Developer", time: 1 }] });
        }, hash);
        await page.locator(`[data-surface="b"] .crow[data-hash="${hash}"]`).waitFor({ timeout: 3000 });
      }
      expect(await page.evaluate(() => window.__beta === document.querySelector('[data-surface="b"]'))).toBe(true);
      expect(await page.locator('[data-directory="b"]').getAttribute("aria-selected")).toBe("true");
    });
  }, 60_000);
}


it("shows the cached surviving file when removing the active file's directory", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "files", [
      { id: "dir-a", source_id: "a", name: "Alpha", is_git: true, status: "ready" },
      { ...added, is_git: true, status: "ready" },
    ]);
    await page.locator('[data-root="a"] [data-path="readme.txt"]').dblclick();
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Kept file"));
    await page.locator('[data-root="b"] [data-path="readme.txt"]').dblclick();
    await page.waitForFunction(() => window.__layoutModules.app.App.route.sourceId === "b" && document.querySelector(".fpdoc")?.textContent.includes("Kept file"));
    await page.evaluate(() => { window.__readCount = window.__calls.filter((call) => call.method === "fs.read").length; });
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "b")));
    await page.waitForFunction(() => window.__layoutModules.app.App.route.sourceId === "a" && document.querySelector(".fpdoc")?.textContent.includes("Kept file"), null, { timeout: 3000 });
    expect(await page.locator('.fpdoc [aria-label="loading"]').count()).toBe(0);
    expect(await page.evaluate(() => window.__calls.filter((call) => call.method === "fs.read").length)).toBe(await page.evaluate(() => window.__readCount));
  });
}, 60_000);

it("pulls the surviving file without adopting another root's cached body on rebind", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "files", [
      { id: "dir-a", source_id: "a", name: "Alpha", is_git: true, status: "ready" },
      { ...added, is_git: true, status: "ready" },
    ]);
    await page.evaluate(() => { window.__holdFileReads = true; });
    await page.locator('[data-root="a"] [data-path="readme.txt"]').dblclick();
    await page.waitForFunction(() => window.__heldReads.length === 1);
    await page.evaluate(() => window.__heldReads[0].resolve({ mime: "text/plain", content_b64: btoa("Alpha contents"), size: 14 }));
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Alpha contents"));
    await page.locator('[data-root="b"] [data-path="readme.txt"]').dblclick();
    await page.waitForFunction(() => window.__heldReads.length === 2);
    await page.evaluate(() => window.__heldReads[1].resolve({ mime: "text/plain", content_b64: btoa("Beta contents"), size: 13 }));
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Beta contents"));
    await page.evaluate(() => { window.__preview = document.querySelector(".fpdoc").firstElementChild; });
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "a")));
    await page.waitForFunction(() => window.__heldReads.length === 3 || document.querySelector(".fpdoc")?.textContent.includes("Alpha contents"));
    expect(await page.locator(".fpdoc").textContent()).toContain("Beta contents");
    expect(await page.evaluate(() => window.__preview === document.querySelector(".fpdoc").firstElementChild)).toBe(true);
    expect(await page.evaluate(() => window.__heldReads.length)).toBe(3);
    expect(await page.evaluate(() => window.__heldReads[2].params)).toMatchObject({ entity_id: "workspace-run", source_id: "b", path: "readme.txt" });
    expect(await page.evaluate(() => window.__layoutModules.app.App.route.sourceId)).toBe("b");
    await page.evaluate(() => window.__heldReads[2].resolve({ mime: "text/plain", content_b64: btoa("Beta refreshed"), size: 14 }));
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Beta refreshed"));
    expect(await page.locator('.fpdoc [aria-label="loading"]').count()).toBe(0);
  });
}, 60_000);

it("restarts a loading surviving file after its root changes cache scope", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "files", [
      { id: "dir-a", source_id: "a", name: "Alpha", is_git: true, status: "ready" },
      { ...added, is_git: true, status: "ready" },
    ]);
    await page.evaluate(() => { window.__holdFileReads = true; });
    await page.locator('[data-root="b"] [data-path="readme.txt"]').dblclick();
    await page.waitForFunction(() => window.__heldReads.length === 1);
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "a")));
    await page.waitForFunction(() => !document.querySelector('[data-root="a"]'));
    await page.evaluate(() => window.__heldReads[0].resolve({ mime: "text/plain", content_b64: btoa("Old scope reply"), size: 15 }));
    await page.waitForFunction(() => window.__heldReads.length === 2, null, { timeout: 3000 });
    expect(await page.evaluate(() => window.__heldReads[1].params.entity_id)).toBe("workspace-run");
    await page.evaluate(() => window.__heldReads[1].resolve({ mime: "text/plain", content_b64: btoa("Rebound file"), size: 12 }));
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("Rebound file"));
    expect(await page.locator('.fpdoc [aria-label="loading"]').count()).toBe(0);
    expect(await page.evaluate(() => window.__layoutModules.app.App.route.sourceId)).toBe("b");
  });
}, 60_000);


it("retires a removed directory's Git dialog and keeps a surviving directory's controller", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "changes");
    await page.evaluate((added) => {
      window.__alphaDirectory = window.__workspace.directories[0];
      return window.__writeWorkspace([...window.__workspace.directories, { ...added, status: "ready" }]);
    }, added);
    await page.locator('[data-directory="b"]').click();
    await page.locator('[data-surface="b"] [data-init-git]').waitFor();
    await page.evaluate(() => { window.__betaButton = document.querySelector('[data-surface="b"] [data-init-git]'); });
    await page.locator('[data-directory="a"]').click();
    await page.locator('[data-surface="a"] [data-init-git]').click();
    await page.locator(".modal-workspace-init").waitFor();
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "a")));
    await page.waitForFunction(() => !document.querySelector('[data-directory="a"]'));
    await page.evaluate(() => window.__writeWorkspace([...window.__workspace.directories, { ...window.__alphaDirectory, id: "dir-a-again" }]));
    await page.locator('[data-directory="a"]').click();
    expect(await page.locator(".modal-workspace-init").count()).toBe(0);
    expect(await page.evaluate(() => window.__betaButton === document.querySelector('[data-surface="b"] [data-init-git]'))).toBe(true);
    await page.locator('[data-surface="a"] [data-init-git]').click();
    await page.locator(".modal-workspace-init").waitFor();
  });
}, 60_000);

it("keeps dirty file edits through a cache scope rebind and its replacement read", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkspace(page, basePath, "files", [
      { id: "dir-a", source_id: "a", name: "Alpha", is_git: true, status: "ready" },
      { ...added, is_git: true, status: "ready" },
    ]);
    await page.evaluate(() => { window.__fileEditable = true; });
    await page.locator('[data-root="b"] [data-path="readme.txt"]').dblclick();
    await page.locator('[data-file-mode="edit"]').click();
    await page.locator('.fpdoc textarea').fill("Unsaved draft");
    await page.evaluate(() => { window.__holdFileReads = true; window.__editor = document.querySelector('.fpdoc textarea'); });
    await page.evaluate(() => window.__writeWorkspace(window.__workspace.directories.filter((dir) => dir.source_id !== "a")));
    await page.waitForFunction(() => window.__heldReads.length === 1, null, { timeout: 3000 });
    await page.evaluate(() => window.__heldReads[0].resolve({ mime: "text/plain", content_b64: btoa("Changed on disk"), size: 15, editable: true, revision: "file-v2" }));
    await page.waitForFunction(() => document.querySelector('.ftab')?.classList.contains("dirty"));
    expect(await page.locator('.fpdoc textarea').inputValue()).toBe("Unsaved draft");
    expect(await page.evaluate(() => window.__editor === document.querySelector('.fpdoc textarea'))).toBe(true);
  });
}, 60_000);
