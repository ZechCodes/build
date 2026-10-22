import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { shellHtml, SHELL_STYLES } from "./chatMenuSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

async function seedFilteredConversation({ lean, hiddenCount }) {
  const { App } = window.__layoutModules.app;
  const { mountAgentRail } = window.__layoutModules.rail;
  const { writeCached } = window.__layoutModules.cache;
  const { greetBridge } = window.__layoutModules.events;
  const { startFeed } = window.__layoutModules.feed;
  const deviceId = "filter-device", entityId = "filter-run", projectId = "filter-project", agentId = "filter-agent";
  App.devices = [{ id: deviceId, name: "Fixture computer" }];
  App.selectedDeviceId = deviceId;
  await greetBridge(async () => ({
    api_version: "3.24.0", capabilities: lean ? ["threads.leanActivity"] : [], push_events: false,
  }), { deviceId });
  await writeCached({ deviceId, entityId, kind: "row", sub: "" }, {
    kind: "project", entity_id: entityId, run_id: entityId, project_id: projectId,
    agents: [{ id: agentId, ordinal: 1, topic: "Project review", provider: "claude_adk",
      state: "idle", working: false, unread_count: 0, read_through_sequence: hiddenCount + 1 }],
  });
  await writeCached({ deviceId, entityId: "", kind: "projects", sub: "" }, [
    { id: projectId, project_id: projectId, name: "Build fixture" },
  ]);
  await startFeed();
  const history = [
    { type: "message", data: { sequence: 1, role: "agent", body: "Here is the project decision you need to review." } },
    ...Array.from({ length: hiddenCount }, (_, index) => ({
      type: "message", data: { sequence: index + 2, role: "user", body: `Worker report ${index + 1}`,
        from_agent: { id: "worker-agent", owner: { kind: "workspace", id: "worker-workspace", name: "Fixture worker" }, topic: "Worker report" } },
    })),
  ];
  window.__filteredPages = [];
  window.__filteredRail = mountAgentRail(document.querySelector("#agent-rail"), {
    kind: "project", deviceId, entityId, projectId, openAgentId: agentId, panelOpen: true,
    call: async (method, params) => {
      if (method === "models.list") return { providers: [] };
      if (method !== "thread.page") return {};
      window.__filteredPages.push(params);
      const eligible = history.filter((item) => !params.before_sequence || item.data.sequence < params.before_sequence);
      const items = eligible.slice(-(params.limit ?? 100));
      return { items, thread_last_sequence: history.length, thread_total: history.length,
        oldest_sequence: items[0]?.data.sequence, has_more: eligible.length > items.length, activity_digests: [] };
    },
  });
}

it.each([
  { bridge: "lean", lean: true, hiddenCount: 23 },
  { bridge: "legacy", lean: false, hiddenCount: 100 },
])("keeps older dialogue reachable when a $bridge cold page is entirely filtered out", async ({ bridge, lean, hiddenCount }) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountLayout(page, shellHtml("mobile"), { basePath, styles: SHELL_STYLES });
    await loadBrowserModules(page, {
      app: "src/app.js", rail: "src/core/agentRail.js", cache: "src/core/localCache.js",
      events: "src/core/changeEvents.js", feed: "src/core/taskFeed.js",
    }, basePath);
    await page.evaluate(seedFilteredConversation, { lean, hiddenCount });
    await page.waitForFunction(() => document.querySelector(".thread-empty")?.textContent === "Nothing at this level.");
    const held = await page.evaluate(async () => (await window.__layoutModules.cache.readCached({
      deviceId: "filter-device", entityId: "filter-run", kind: "thread", sub: "filter-agent",
    })).value);
    expect(held.items).toHaveLength(hiddenCount);
    expect(held.olderItemsRemain).toBe(true);
    expect(await page.evaluate(() => window.__filteredPages)).toEqual([{
      entity_id: "filter-run", agent_id: "filter-agent", limit: hiddenCount,
      ...(lean ? { activity: "newest" } : {}),
    }]);
    const earlier = page.getByRole("button", { name: "Show earlier messages" });
    await captureLayout(page, `filtered-cold-${bridge}.png`);
    expect(await earlier.isVisible()).toBe(true);
    await earlier.click();
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Here is the project decision"));
    const pages = await page.evaluate(() => window.__filteredPages);
    expect(pages).toHaveLength(2);
    expect(pages[1]).toEqual({
      entity_id: "filter-run", agent_id: "filter-agent", before_sequence: 2,
      ...(lean ? { activity: "newest" } : {}),
    });
    expect(await page.locator(".thread-empty").count()).toBe(0);
    expect(await earlier.count()).toBe(0);
    await captureLayout(page, `filtered-older-reply-${bridge}.png`);
  }, { width: 390, height: 844 });
}, 30_000);
