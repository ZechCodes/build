import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
const fixture = JSON.parse(readFileSync(new URL("../../../fixtures/api/v1/workspace.set_locked.json", import.meta.url), "utf8"));
it("clicks the workspace lock against the fixture bridge and paints only its cached push", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div id="toolbar"></div>', { basePath });
    await loadBrowserModules(page, {
      app: "src/app.js", toolbar: "src/core/toolbar.js", feed: "src/core/taskFeed.js",
      cache: "src/core/localCache.js", support: "src/core/workspaceLockSupport.js", contexts: "src/core/deviceContexts.js",
    }, basePath);
    await page.evaluate(async (fixture) => {
      const { app, toolbar, feed, cache, support, contexts } = window.__layoutModules;
      const deviceId = "fixture-398";
      window.__lockWorkspace = { ...fixture.result, name: "Workspace lock", managed: true, locked: false };
      const project = { id: window.__lockWorkspace.project_id, project_id: window.__lockWorkspace.project_id, name: "Build", path: "/source" };
      app.App.route = { name: "workspace", deviceId, projectId: project.id, workspaceId: window.__lockWorkspace.workspace_id };
      app.App.devices = [{ id: deviceId, name: "Workshop" }];
      window.__lockRequests = [];
      contexts.adoptDeviceSession({ deviceId, close: () => {}, peer: () => {}, onCarrier: () => {}, onPush: () => {},
        call: async (method, params) => {
          if (method !== fixture.method) return {};
          if (params.workspace_id !== fixture.params.workspace_id || typeof params.locked !== "boolean") throw new Error("invalid_params");
          window.__lockRequests.push(params);
          // The RPC result intentionally lands before its push.
          return { ...fixture.result, locked: params.locked };
        },
      });
      await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
      await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [project]);
      await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, [window.__lockWorkspace]);
      await support.rememberWorkspaceLockSupport(deviceId, { workspaces: { setLocked: true } });
      window.__lockPush = async () => {
        window.__lockWorkspace.locked = window.__lockRequests.at(-1).locked;
        await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, [window.__lockWorkspace]);
      };
      await feed.startFeed();
      await toolbar.initToolbar();
    }, fixture);
    const lock = page.locator('[data-workspace-lock]');
    await lock.waitFor({ timeout: 5000 });
    expect(await lock.getAttribute("aria-label")).toBe("Lock workspace");
    const geometry = await lock.evaluate((button) => ({
      svgHeight: button.querySelector("svg").getBoundingClientRect().height,
      fontSize: parseFloat(getComputedStyle(document.querySelector('.tb-name')).fontSize),
      afterName: button.previousElementSibling.matches('[data-select="workspace"]'),
    }));
    expect(geometry.afterName).toBe(true);
    expect(Math.abs(geometry.svgHeight - geometry.fontSize)).toBeLessThanOrEqual(1);
    await captureLayout(page, "workspace-nav-unlocked.png");
    await lock.click();
    await page.waitForFunction(() => window.__lockRequests.length === 1 && !document.querySelector('[data-workspace-lock]').disabled);
    expect(await lock.getAttribute("aria-label")).toBe("Lock workspace");
    await page.evaluate(() => window.__lockPush());
    await page.waitForFunction(() => document.querySelector('[data-workspace-lock]').getAttribute("aria-label") === "Unlock workspace");
    await captureLayout(page, "workspace-nav-locked.png");
    await lock.focus();
    await page.keyboard.press("Space");
    await page.waitForFunction(() => window.__lockRequests.length === 2);
    expect(await page.evaluate(() => window.__lockRequests.at(-1).locked)).toBe(false);
    await page.evaluate(() => window.__lockPush());
    await page.waitForFunction(() => document.querySelector('[data-workspace-lock]').getAttribute("aria-label") === "Lock workspace");
    await page.waitForFunction(() => !document.querySelector('[data-workspace-lock]').disabled);
    expect(await lock.evaluate((button) => document.activeElement === button)).toBe(true);
    await page.keyboard.press("Space");
    await page.waitForFunction(() => window.__lockRequests.length === 3);
    expect(await page.evaluate(() => window.__lockRequests.at(-1).locked)).toBe(true);
    await page.evaluate(() => window.__lockPush());
    await page.waitForFunction(() => document.querySelector('[data-workspace-lock]').getAttribute("aria-label") === "Unlock workspace");
    expect(await lock.evaluate((button) => document.activeElement === button)).toBe(true);
    await page.evaluate(async () => {
      const { toolbar, feed, contexts } = window.__layoutModules;
      await toolbar.stopToolbar(); feed.stopFeed(); contexts.resetDeviceContexts();
    });
  }, { plugins: [deviceShim] });
}, 60_000);
