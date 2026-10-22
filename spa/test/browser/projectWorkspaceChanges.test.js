import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { SHELL_HTML, SHELL_STYLES } from "./filesExplorerSeed.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); });
afterAll(async () => { await session?.close(); });

const PATCH = "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -7 +7 @@\n-old wording\n+shared source wording\n";
const COMMIT = "d".repeat(40);

async function mountProject(page, basePath, options = {}) {
  page.setDefaultTimeout(5000);
  await mountLayout(page, `${SHELL_HTML}<aside id="agent-rail">Kept project chat</aside><div id="console">Kept console</div>`, { basePath, styles: SHELL_STYLES });
  await loadBrowserModules(page, {
    app: "src/app.js", project: "src/views/projectView.js", contexts: "src/core/deviceContexts.js",
    cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", adapter: "src/core/bridgeApi/v1/index.js",
    support: "src/core/projectChangesSupport.js", filesSupport: "src/core/projectFilesSupport.js", directory: "src/core/directoryScope.js", changes: "src/core/changeEvents.js",
  }, basePath);
  await page.evaluate(seedProject, { theme: "dark", sourceId: "code", supported: true, greeting: true, patch: PATCH, ...options });
}

async function seedProject({ theme, sourceId, supported, greeting, commit, patch, lateProject, removedSource }) {
  document.documentElement.dataset.theme = theme;
  document.head.insertAdjacentHTML("afterbegin", '<meta name="viewport" content="width=device-width, initial-scale=1">');
  const { app, project, contexts, cache, feed, adapter, support, filesSupport, directory, changes } = window.__layoutModules;
  const deviceId = "project-changes-device";
  const status = { branch: "feature", status_key: "fixture-status", files: [{ path: "README.md", staged: "none", index_status: "M", worktree_status: "M", content_key: "fixture-content", added: 1, deleted: 1 }], stat: { files_changed: 1, insertions: 1, deletions: 1 }, repo_state: "clean", ahead: 0, behind: 0, stash_count: 0 };
  const history = { hash: "d".repeat(40), short: "ddddddd", subject: "Document the project workspace", author: "Developer", time: Math.floor(Date.now() / 1000) - 90 };
  window.__calls = [];
  window.__project = { id: "p", project_id: "p", deviceId, projectKey: `${deviceId}/p`, name: "Build", path: "/code", sources: [
    { id: "code", name: "Code", path: "/code", is_git: true, base_branch: "main", sync_base: true, sync: { status: "skipped", reason: "uncommitted changes" } },
    { id: "docs", name: "Docs", path: "/docs", is_git: true, base_branch: "main", sync_base: true },
    { id: "assets", name: "Assets", path: "/assets", is_git: false },
  ] };
  const replies = {
    "git.status": () => status,
    "git.log": () => ({ commits: [history] }),
    "git.diff": () => ({ patch, files: [{ path: "README.md", content_key: "fixture-content", patch, truncated: false }] }),
    "git.show": () => ({ ...history, patch, email: "developer@example.test", stat: { files_changed: 1, insertions: 1, deletions: 1 } }),
    "git.refs": () => ({ current: { name: "feature", kind: "branch" }, refs: [{ name: "feature", full_ref: "refs/heads/feature", kind: "branch", current: true }, { name: "main", full_ref: "refs/heads/main", kind: "branch" }] }),
    "git.commit": () => ({ status: { ...status, files: [] } }),
    "fs.tree": (params) => ({ path: params.path, entries: [{ name: "README.md", kind: "file", size: 50 }] }),
    "fs.read": () => ({ path: "README.md", mime: "text/plain", content_b64: btoa("one\ntwo\nthree\nfour\nfive\nsix\nshared source wording"), size: 50, editable: true, encoding: "utf-8", revision: "r1" }),
  };
  const hello = { api_version: "3.24.0", capabilities: supported ? ["git.projectSources", "fs.projectSources", "sources.syncBase", "sources.syncBasePath"] : [] };
  replies["session.hello"] = () => hello;
  const call = async (method, params) => { window.__calls.push({ method, params }); return replies[method]?.(params) || {}; };
  const context = contexts.adoptDeviceSession({ deviceId, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
  if (greeting) {
    contexts.adoptBridgeSelection(context, { version: "3.21.0" }, adapter.create(call, hello));
    await changes.greetBridge(call, { deviceId });
  }
  await support.rememberProjectChangesSupport(deviceId, { git: { projectSources: supported }, projects: { syncBase: supported, syncBasePath: supported } });
  await filesSupport.rememberProjectFilesSupport(deviceId, { fs: { projectSources: true } });
  app.App.devices = [{ id: deviceId, name: "Fixture device" }];
  app.App.route = { name: "project", deviceId, projectId: "p", tab: "changes", sourceId, commit: commit || null };
  await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
  window.__writeProject = async (sources) => {
    window.__project = { ...window.__project, sources };
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [window.__project]);
  };
  if (!lateProject) await window.__writeProject(window.__project.sources.filter((source) => source.id !== removedSource));
  for (const source of window.__project.sources.filter((source) => source.is_git)) {
    const scope = supported ? { project_id: "p", source_id: source.id, source_path: source.path } : { project_id: "p" };
    const entityId = directory.directoryCacheId(scope);
    await cache.writeCached({ deviceId, entityId, kind: "status" }, status);
    await cache.writeCached({ deviceId, entityId, kind: "log" }, { commits: [history] });
  }
  await feed.startFeed();
  await project.renderProject();
  window.__chat = document.querySelector("#agent-rail");
  window.__console = document.querySelector("#console");
}

it("opens a source-qualified commit deep link and carries its diff location to Files", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath, { sourceId: "docs", commit: COMMIT });
    await page.locator(`[data-surface="docs"] .commitheader[data-hash="${COMMIT}"]`).waitFor({ timeout: 3000 });
    expect(await page.locator('[data-directory="docs"]').getAttribute("aria-selected")).toBe("true");
    expect(await page.evaluate(() => window.__calls.find((call) => call.method === "git.show").params)).toMatchObject({ project_id: "p", source_id: "docs", source_path: "/docs", hash: COMMIT });
    await page.locator('[data-surface="docs"] [data-open-file="README.md"]').click();
    await page.waitForFunction(() => document.querySelector(".fpdoc")?.textContent.includes("shared source wording"));
    expect(await page.evaluate(() => window.__layoutModules.app.App.route)).toMatchObject({ tab: "files", sourceId: "docs", file: "README.md", line: 7 });
    expect(await page.evaluate(() => location.hash)).toContain("/files?source=docs&path=README.md&line=7");
    expect(await page.evaluate(() => document.querySelector("#agent-rail") === window.__chat && document.querySelector("#console") === window.__console)).toBe(true);
  });
}, 60_000);

it("resolves an explicit source and commit when the cached project arrives after mount", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath, { sourceId: "docs", commit: COMMIT, lateProject: true });
    await page.waitForFunction(() => document.querySelector("[data-project-source-context]")?.textContent);
    expect(await page.evaluate(() => window.__layoutModules.app.App.route)).toMatchObject({ sourceId: "docs", commit: COMMIT });
    expect(await page.locator('[data-surface="code"]').count()).toBe(0);
    await page.evaluate(() => window.__writeProject(window.__project.sources));
    await page.locator(`[data-surface="docs"] .commitheader[data-hash="${COMMIT}"]`).waitFor({ timeout: 3000 });
    expect(await page.locator('[data-directory="docs"]').getAttribute("aria-selected")).toBe("true");
    expect(await page.evaluate(() => location.hash)).toContain(`/changes?source=docs&commit=${COMMIT}`);
    expect(await page.evaluate(() => window.__calls.find((call) => call.method === "git.show").params)).toMatchObject({ source_id: "docs", source_path: "/docs", hash: COMMIT });
  });
}, 60_000);

it.each([390, 1280])("recovers a fresh removed-source bookmark without blocking surviving tabs at %i px", async (width) => {
  await session.withPage(async ({ page, basePath }) => {
    await page.setViewportSize({ width, height: 900 });
    await mountProject(page, basePath, { sourceId: "docs", commit: COMMIT, removedSource: "docs" });
    await page.locator('[data-surface="code"] .csinput').first().waitFor();
    expect(await page.locator('[data-directory="code"]').getAttribute("aria-selected")).toBe("true");
    expect(await page.locator('[data-directory="assets"]').count()).toBe(1);
    expect(await page.locator('[data-project-missing-source]').textContent()).toMatch(/docs.*unavailable/);
    expect(await page.evaluate(() => window.__layoutModules.app.App.route)).toMatchObject({ sourceId: "docs", commit: COMMIT });
    expect(await page.evaluate(() => window.__calls.filter((call) => call.method === "git.show"))).toEqual([]);
    await page.evaluate(() => window.__writeProject(window.__project.sources));
    expect(await page.locator('[data-project-missing-source]').isVisible()).toBe(true);
    if (width === 390) await page.locator('[data-project-use-source]').click();
    else await page.locator('[data-directory="assets"]').click();
    expect(await page.evaluate(() => {
      const route = window.__layoutModules.app.App.route;
      return { sourceId: route.sourceId, commit: route.commit ?? null };
    })).toEqual({ sourceId: width === 390 ? "code" : "assets", commit: null });
    expect(await page.evaluate(() => location.hash)).not.toContain("commit=");
    expect(await page.locator('[data-project-missing-source]').isVisible()).toBe(false);
    expect(await page.evaluate(() => window.__calls.filter((call) => call.method === "git.show"))).toEqual([]);
  });
}, 60_000);

it.each(["empty", "promoted"])("opens the pending exact commit after survivor reconciliation (%s)", async (roster) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath, { sourceId: "docs", commit: COMMIT, removedSource: "docs" });
    await page.locator('[data-surface="code"] .csinput').first().waitFor();
    if (roster === "empty") {
      await page.evaluate(() => window.__writeProject([]));
      await page.waitForFunction(() => !document.querySelector('[data-surface="code"]'));
      expect(await page.evaluate(() => window.__layoutModules.app.App.route)).toMatchObject({ sourceId: "docs", commit: COMMIT });
    }
    await page.evaluate(() => window.__writeProject([...window.__project.sources.filter((source) => source.id !== "code"),
      { id: "docs", name: "Docs", path: "/docs", is_git: true, base_branch: "main" }]));
    await page.locator(`[data-surface="docs"] .commitheader[data-hash="${COMMIT}"]`).waitFor();
    expect(await page.locator('[data-directory="docs"]').getAttribute("aria-selected")).toBe("true");
    expect(await page.evaluate(() => location.hash)).toContain(`/changes?source=docs&commit=${COMMIT}`);
    expect(await page.locator('[data-project-missing-source]').isVisible()).toBe(false);
    expect(await page.evaluate(() => window.__calls.find((call) => call.method === "git.show").params))
      .toMatchObject({ source_id: "docs", source_path: "/docs", hash: COMMIT });
  });
}, 60_000);

it("opens a commit-only deep link on the default source", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath, { sourceId: null, commit: COMMIT });
    await page.locator(`[data-surface="code"] .commitheader[data-hash="${COMMIT}"]`).waitFor({ timeout: 3000 });
    expect(await page.evaluate(() => window.__layoutModules.app.App.route)).toMatchObject({ tab: "changes", sourceId: "code", commit: COMMIT });
    expect(await page.evaluate(() => window.__calls.find((call) => call.method === "git.show").params)).toMatchObject({ source_id: "code", source_path: "/code", hash: COMMIT });
  });
}, 60_000);

it("keeps source commit drafts across tabs and faces and commits only the captured source", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath);
    await page.locator('[data-surface="code"] .csinput').fill("Code draft");
    await page.locator('[data-directory="docs"]').click();
    await page.locator('[data-surface="docs"] .csinput').fill("Docs draft");
    await page.locator('[data-tab="files"]').click();
    await page.locator('[data-tab="changes"]').click();
    expect(await page.locator('[data-surface="docs"] .csinput').inputValue()).toBe("Docs draft");
    await page.locator('[data-directory="code"]').click();
    expect(await page.locator('[data-surface="code"] .csinput').inputValue()).toBe("Code draft");
    await page.locator('[data-directory="docs"]').click();
    await page.locator('[data-surface="docs"] .csbox-actions button').filter({ hasText: "Commit" }).click();
    await page.waitForFunction(() => window.__calls.some((call) => call.method === "git.commit"));
    const mutations = await page.evaluate(() => window.__calls.filter((call) => call.method === "git.stage" || call.method === "git.commit"));
    expect(mutations).toEqual([
      { method: "git.stage", params: { project_id: "p", source_id: "docs", source_path: "/docs", paths: ["README.md"] } },
      { method: "git.commit", params: { project_id: "p", source_id: "docs", source_path: "/docs", message: "Docs draft" } },
    ]);
    await page.locator('[data-directory="code"]').click();
    expect(await page.locator('[data-surface="code"] .csinput').inputValue()).toBe("Code draft");
    expect(await page.evaluate(() => document.querySelector("#agent-rail") === window.__chat)).toBe(true);
  });
}, 60_000);

it("reconciles source rename, addition, removal and movement without losing unrelated drafts", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath, { sourceId: "docs" });
    await page.locator('[data-surface="docs"] .csinput').fill("Surviving docs draft");
    await page.evaluate(() => { window.__docsField = document.querySelector('[data-surface="docs"] .csinput'); });
    await page.evaluate(() => window.__writeProject(window.__project.sources.map((source) => source.id === "docs" ? { ...source, name: "Documentation" } : source)));
    await page.waitForFunction(() => document.querySelector('[data-directory="docs"]')?.textContent.includes("Documentation"));
    await page.evaluate(() => window.__writeProject([...window.__project.sources, { id: "extras", name: "Extras", path: "/extras", is_git: false }]));
    await page.locator('[data-directory="extras"]').waitFor({ timeout: 3000 });
    await page.evaluate(() => window.__writeProject(window.__project.sources.filter((source) => source.id !== "extras")));
    await page.waitForFunction(() => !document.querySelector('[data-directory="extras"]'));
    await page.locator('[data-directory="code"]').click();
    await page.locator('[data-surface="code"] .csinput').fill("Old code location draft");
    await page.locator('[data-directory="docs"]').click();
    await page.evaluate(() => window.__writeProject(window.__project.sources.map((source) => source.id === "code" ? { ...source, path: "/moved-code" } : source)));
    expect(await page.locator('[data-surface="docs"] .csinput').inputValue()).toBe("Surviving docs draft");
    expect(await page.evaluate(() => document.querySelector('[data-surface="docs"] .csinput') === window.__docsField && document.querySelector("#agent-rail") === window.__chat)).toBe(true);
    await page.locator('[data-directory="code"]').click();
    await page.waitForFunction(() => window.__calls.some((call) => call.method === "git.status" && call.params.source_path === "/moved-code"));
    expect(await page.locator('[data-surface="code"] .csinput').inputValue()).toBe("");
    const movedDraft = page.locator('[data-recovered-draft="code"] textarea');
    await movedDraft.waitFor({ timeout: 3000 });
    expect(await movedDraft.inputValue()).toBe("Old code location draft");
    expect(await movedDraft.getAttribute("readonly")).not.toBeNull();
    await page.evaluate(() => window.__writeProject(window.__project.sources.filter((source) => source.id !== "docs")));
    const removedDraft = page.locator('[data-recovered-draft="docs"] textarea');
    await removedDraft.waitFor({ timeout: 3000 });
    expect(await removedDraft.inputValue()).toBe("Surviving docs draft");
    // A fresh controller recovers from the persisted UI store, not a kept DOM.
    await page.evaluate(async () => {
      const { app, project } = window.__layoutModules;
      app.App.viewDispose?.();
      app.App.viewDispose = null;
      app.App.route = { ...app.App.route, tab: "changes" };
      await project.renderProject();
    });
    await page.locator('[data-recovered-draft="docs"] textarea').waitFor({ timeout: 3000 });
    expect(await page.locator('[data-recovered-draft="code"] textarea').inputValue()).toBe("Old code location draft");
    expect(await page.locator('[data-recovered-draft="docs"] textarea').inputValue()).toBe("Surviving docs draft");
  });
}, 60_000);

it("paints cached source support before a current greeting and refuses new source parameters", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath, { sourceId: "docs", greeting: false });
    await page.locator('[data-surface="docs"] .csinput').waitFor({ timeout: 3000 });
    expect(await page.locator(".project-changes-context").textContent()).toContain("Original sources · shared checkout");
    expect(await page.evaluate(() => window.__calls.filter((call) => call.method.startsWith("git.")).length)).toBe(0);
  });
}, 60_000);

it("retains the legacy first-source pane and explains additional sources on an old bridge", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProject(page, basePath, { supported: false });
    await page.locator('[data-surface="code"] .csinput').waitFor({ timeout: 3000 });
    await page.locator('[data-directory="docs"]').click();
    expect(await page.locator('[data-surface="docs"]').textContent()).toContain("Update this device");
    expect(await page.evaluate(() => window.__calls.some((call) => call.params?.source_id && call.method.startsWith("git.")))).toBe(false);
  });
}, 60_000);

for (const width of [390, 1280]) {
  for (const theme of ["light", "dark"]) {
    it(`shows project workspace Changes at ${width}px in ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await mountProject(page, basePath, { theme });
        expect(await page.evaluate(() => document.characterSet)).toBe("UTF-8");
        await page.locator('[data-directory="code"]').waitFor({ timeout: 3000 });
        expect(await page.locator('[data-directory]').count()).toBe(3);
        expect(await page.locator('[data-tab="changes"]').getAttribute("aria-selected")).toBe("true");
        expect(await page.locator(".project-changes-context").textContent()).toContain("Original sources · shared checkout");
        expect(await page.locator(".project-changes-context").textContent()).toContain("main");
        await page.locator('[data-surface="code"] .csinput').waitFor({ timeout: 3000 });
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        await captureLayout(page, `534-project-changes-${width}-${theme}.png`);
        await page.locator('[data-directory="assets"]').click();
        expect(await page.locator('[data-surface="assets"]').textContent()).toContain("has no Git repository");
        expect(await page.locator(".project-changes-context").textContent()).toContain("This source is not a Git repository.");
        expect(await page.locator('[data-surface="assets"] [data-init-git]').count()).toBe(0);
      }, { width, height: 900 });
    }, 60_000);
  }
}
