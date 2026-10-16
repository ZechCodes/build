import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const body = readFileSync(new URL("../../index.html", import.meta.url), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const NOW = new Date("2026-10-16T12:00:00Z");

it.each(["inbox", "projects"])("dispatches an aged capture through a real editor click in %s", async (view) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.clock.setFixedTime(NOW);
    await mountLayout(page, body, { basePath });
    await loadBrowserModules(page, { app: "src/app.js", cache: "src/core/localCache.js",
      feed: "src/core/taskFeed.js", inbox: "src/core/inboxView.js", contexts: "src/core/deviceContexts.js",
      adapter: "src/core/bridgeApi/v1/index.js" }, basePath);
    await page.evaluate(async (view) => {
      const { app, cache, feed, inbox, contexts, adapter } = window.__layoutModules;
      const deviceId = "capture-editor-device";
      const projectId = "capture-project";
      const projectKey = `${deviceId}/${projectId}`;
      const capture = { kind: "capture", capture_id: "editor-capture", title: "Preserve this editor",
        project_id: projectId, projectKey, deviceId, state: "routed", created_at: new Date().toISOString(),
        routing: { project_id: projectId, kind: "branch", target_id: "build/original" } };
      Object.assign(app.App, { devices: [{ id: deviceId, name: "Fixture device", status: "online" }],
        route: { name: "inbox" }, selectedDeviceId: deviceId, deviceFilter: null, gated: false });
      window.__reroutes = [];
      const call = async (method, params) => {
        if (method !== "capture.reroute") return {};
        window.__reroutes.push(params);
        return new Promise(() => {});
      };
      const context = contexts.adoptDeviceSession({ deviceId, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
      contexts.adoptBridgeSelection(context, { version: "3.21.0" },
        adapter.create(call, { api_version: "3.21.0", capabilities: [] }));
      await cache.writeCached({ deviceId, entityId: "", kind: "projects" },
        [{ id: projectId, project_id: projectId, projectKey, deviceId, name: "Build" }]);
      await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { items: [capture], runs: [] });
      inbox.setInboxView(view);
      inbox.mountInboxList();
      await feed.startFeed();
    }, view);
    const row = page.locator('[data-key="capture:editor-capture"]');
    await row.locator("[data-capture-reroute]").evaluate((button) => button.click());
    await row.locator("[data-reroute-branch-open]").evaluate((button) => button.click());
    const field = row.locator("[data-reroute-branch]");
    await field.fill("build/typed-draft");
    await page.clock.setFixedTime(new Date(NOW.getTime() + 24 * 3600_000));
    await page.evaluate(() => window.dispatchEvent(new Event("pageshow")));
    expect(await field.evaluate((element) => document.activeElement === element && !element.closest(".inbox-recent"))).toBe(true);

    // A normal click transfers focus on pointerdown. Give the queued blur
    // reconciliation a turn before pointerup generates the actual click.
    const dispatch = row.locator("[data-reroute-kind='branch']");
    const box = await dispatch.boundingBox();
    const hit = await page.evaluate(({ x, y, width, height }) => document.elementFromPoint(x + width / 2, y + height / 2)?.outerHTML, box);
    expect(hit).toContain("data-reroute-kind");
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.evaluate(() => new Promise((resolve) => queueMicrotask(resolve)));
    expect(await dispatch.isVisible()).toBe(true);
    await page.mouse.up();
    await page.waitForFunction(() => window.__reroutes.length === 1);
    expect(await page.evaluate(() => window.__reroutes[0])).toEqual({ capture_id: "editor-capture",
      project_id: "capture-project", kind: "branch", branch: "build/typed-draft" });
    // Closing the submitted editor releases the age hold, even while its RPC
    // is pending and there has been no feed update.
    await row.waitFor({ state: "detached" });
    expect(await page.locator("[data-recent-toggle]").getAttribute("aria-expanded")).toBe("false");
  });
}, 60_000);
