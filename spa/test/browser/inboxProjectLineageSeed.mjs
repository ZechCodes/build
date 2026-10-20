import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";
import { deviceId, entityId, pinId, projectId, projectLineageFixture } from "../inboxProjectLineageFixture.js";

/** Production inbox and chat rail, backed entirely by IndexedDB fixtures. */
export async function mountProjectLineageInbox(page, basePath, { fixture = projectLineageFixture() } = {}) {
  await mountLayout(page, `<div id="shell"><aside id="inbox-rail" aria-label="Inbox">
    <div class="inbox-head">Inbox</div><div id="inbox-list"></div></aside><div id="view">
    <header id="toolbar">Build</header><div id="view-body"><main id="root"></main>
    <aside id="agent-rail" aria-label="Agents"></aside></div><div id="console-region"></div></div></div>`, { basePath });
  await loadBrowserModules(page, { app: "src/appState.js", cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js", inbox: "src/core/inboxView.js", rail: "src/core/agentRail.js",
    fixture: "test/inboxProjectLineageFixture.js" }, basePath);
  await page.evaluate(async ({ fixture, deviceId, projectId, entityId, pinId }) => {
    const { app, cache, feed, inbox, rail, fixture: helpers } = window.__layoutModules;
    app.App.devices = [{ id: deviceId, name: "Fixture device" }];
    app.App.selectedDeviceId = deviceId;
    app.App.route = { name: "project", deviceId, projectId };
    window.__projectLineageFixture = fixture;
    await helpers.writeProjectLineageFixture(cache, fixture);
    inbox.mountInboxList();
    await feed.startFeed();
    window.__projectLineageRail = rail.mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId, projectId, entityId, openAgentId: pinId, panelOpen: true,
      call: async (method) => method === "models.list"
        ? { default_provider: "codex", providers: [{ id: "codex", label: "Codex", models: [], efforts: [] }] }
        : { items: [] },
    });
  }, { fixture, deviceId, entityId, pinId, projectId });
  await page.locator(".inbox-project-agent").waitFor();
  await page.locator(".rail-head").waitFor();
  await page.waitForFunction(() => document.querySelector("#agent-rail .surface-pill-count")?.textContent);
  await page.evaluate(() => document.fonts.ready);
}

export async function stopProjectLineageChild(page, index) {
  await page.evaluate(async (index) => {
    const { cache } = window.__layoutModules;
    const child = window.__projectLineageFixture.children[index];
    await cache.writeCached({ deviceId: child.deviceId, entityId: child.entity_id, kind: "row" }, {
      ...child, agents: [{ ...child.agents[0], working: false }],
    });
  }, index);
}
