import { beforeAll, afterAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { shellHtml, SHELL_STYLES } from "./chatMenuSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

async function seedLeanThread(warm) {
  const { App } = window.__layoutModules.app;
  const { readCached, writeCached } = window.__layoutModules.cache;
  const { threadWindow } = window.__layoutModules.sync;
  const { greetBridge } = window.__layoutModules.events;
  const { mountAgentRail } = window.__layoutModules.rail;
  const { startFeed } = window.__layoutModules.feed;
  const { stampWorkspace } = window.__layoutModules.merge;
  const deviceId = "lean-device";
  const entityId = "lean-run";
  const agentId = "lean-agent";
  const activity = (sequence) => ({ type: "event", data: { sequence, event: "tool_use", summary: `Read a${sequence}.js` } });
  const message = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });
  App.devices = [{ id: deviceId, name: "This computer" }];
  App.selectedDeviceId = deviceId;
  await greetBridge(async () => ({ api_version: "3.24.0", capabilities: ["threads.leanActivity"], push_events: false }), { deviceId });
  const threadAddress = { deviceId, entityId, kind: "thread", sub: agentId };
  await writeCached({ deviceId, entityId, kind: "row", sub: "" }, {
    kind: "workspace", entity_id: entityId, project_id: "lean-project", workspace_id: "lean-workspace",
    agents: [{ id: agentId, ordinal: 1, topic: "Lean activity", provider: "codex_app_server", state: "idle", working: false, unread_count: 0 }],
  });
  const closedThread = {
    items: [message(1, "Start"), activity(100), message(101, "Done")], deliveredSequence: 101,
    activityDigests: [{ from_sequence: 2, through_sequence: 100, rows: 99, tool_calls: 99 }],
  };
  await writeCached(threadAddress, warm ? {
    items: [message(1, "Start"), activity(2)], deliveredSequence: 2,
    activityDigests: [{ from_sequence: 2, through_sequence: 2, rows: 1, tool_calls: 1 }],
  } : closedThread);
  window.__leanCatchUp = async () => writeCached(threadAddress, threadWindow((await readCached(threadAddress)).value, {
    items: [activity(100), message(101, "Done")], thread_last_sequence: 101, has_more: true,
    activity_digests: closedThread.activityDigests,
  }, { newest: true, lean: true }));
  window.__updatedLeanTail = (finished = false) => writeCached(threadAddress, {
    items: [message(1, "Start"),
      { ...activity(2), data: { ...activity(2).data, updated_sequence: 101, outcome: "ok" } },
      { ...activity(100), data: { ...activity(100).data, ...(finished ? { updated_sequence: 102, outcome: "ok", summary: "Read a100.js\n→ Tool finished" } : {}) } },
    ], deliveredSequence: finished ? 102 : 101, activityDigests: closedThread.activityDigests,
  });
  await writeCached({ deviceId, entityId: "", kind: "projects", sub: "" }, [{ id: "lean-project", project_id: "lean-project", name: "Lean" }]);
  await writeCached({ deviceId, entityId: "", kind: "workspaces", sub: "" }, [
    stampWorkspace({ id: "lean-workspace", project_id: "lean-project", entity_id: entityId, name: "lean-thread" }, deviceId),
  ]);
  await startFeed();
  window.__activityCalls = [];
  window.__pendingActivity = [];
  window.__answerActivity = (items, hasMore = false) => window.__pendingActivity.shift().resolve({
    items: items.map(activity), has_more: hasMore, oldest_sequence: items[0],
  });
  window.__refuseActivity = () => window.__pendingActivity.shift().reject(new Error("device could not answer"));
  // A cache announcement while the fetch is pending must retain its throbber.
  window.__repaintLeanThread = () => writeCached(threadAddress, {
    items: [message(1, "Start again"), activity(100), message(101, "Done")], deliveredSequence: 101,
    activityDigests: [{ from_sequence: 2, through_sequence: 100, rows: 99, tool_calls: 99 }],
  });
  window.__leanRail = mountAgentRail(document.querySelector("#agent-rail"), {
    kind: "workspace", deviceId, entityId, projectId: "lean-project", workspaceId: "lean-workspace", openAgentId: agentId, panelOpen: true,
    call: async (method, params) => {
      if (method === "models.list") return { providers: [] };
      if (method !== "thread.activity") return {};
      window.__activityCalls.push(params);
      return new Promise((resolve, reject) => window.__pendingActivity.push({ resolve, reject }));
    },
  });
}

async function mountLeanThread(page, basePath, { warm = false } = {}) {
  page.setDefaultTimeout(5000);
  await mountLayout(page, shellHtml(page.viewportSize().width <= 760 ? "phone" : "desktop"), { basePath, styles: SHELL_STYLES });
  await loadBrowserModules(page, {
    app: "src/app.js", rail: "src/core/agentRail.js", cache: "src/core/localCache.js", events: "src/core/changeEvents.js",
    feed: "src/core/taskFeed.js", merge: "src/core/feedMerge.js",
    sync: "src/core/threadSync.js",
  }, basePath);
  await page.evaluate(seedLeanThread, warm);
  await page.locator(".thread-activity-group-head").waitFor();
}

it.each([{ width: 1180, height: 840 }, { width: 390, height: 844 }])("opens a lean run behind the existing throbber at $width pixels", async (viewport) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountLeanThread(page, basePath);
    const head = page.locator(".thread-activity-group-head");
    const spinner = page.locator('.thread-activity-group [role="status"] .throbber');
    expect(await head.locator(".thread-activity-count").textContent()).toBe("99");
    expect(await head.locator(".thread-activity-preview").textContent()).toBe("Read a100.js");
    // Measure in the same DOM turn as the click, before fold persistence.
    expect(await head.evaluate((node) => { node.click(); return !!document.querySelector('[role="status"] .throbber'); })).toBe(true);
    await page.waitForFunction(() => window.__activityCalls.length === 1);
    expect(await spinner.isVisible()).toBe(true);
    expect(await spinner.evaluate((node) => getComputedStyle(node).animationName)).toBe("throb");
    await page.evaluate(() => window.__repaintLeanThread());
    expect(await spinner.isVisible()).toBe(true);
    await captureLayout(page, `lean-activity-loading-${viewport.width}.png`);

    // Close and reopen while loading: one shared fetch and one pending state.
    await head.click();
    await page.waitForFunction(() => !document.querySelector(".thread-activity-group").open);
    await head.click();
    await spinner.waitFor();
    expect(await page.evaluate(() => window.__activityCalls.length)).toBe(1);
    await page.evaluate(() => window.__answerActivity([50, 100], true));
    await page.waitForFunction(() => window.__activityCalls.length === 2);
    expect(await spinner.isVisible()).toBe(true);
    expect(await page.evaluate(() => window.__activityCalls[1].before_sequence)).toBe(50);
    await page.evaluate(() => window.__answerActivity([2, 3]));
    await spinner.waitFor({ state: "detached" });
    expect(await page.locator(".thread-activity-group-list > .thread-activity").evaluateAll((rows) => rows.map((row) => row.dataset.sequence))).toEqual(["2", "3", "50", "100"]);
    await captureLayout(page, `lean-activity-open-${viewport.width}.png`);
    await head.click();
    await page.waitForFunction(() => !document.querySelector(".thread-activity-group").open);
    await head.click();
    await page.waitForFunction(() => document.querySelectorAll(".thread-activity-group-list > .thread-activity").length === 4);
    expect(await page.evaluate(() => window.__activityCalls.length)).toBe(2);
  }, viewport);
}, 30_000);

it("fetches missing interior rows after a warm lean catch-up closes the run", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountLeanThread(page, basePath, { warm: true });
    await page.evaluate(() => window.__leanCatchUp());
    await page.waitForFunction(() => document.querySelector(".thread-activity-count")?.textContent === "99");
    const head = page.locator(".thread-activity-group-head");
    expect(await head.evaluate((node) => { node.click(); return !!document.querySelector('[role="status"] .throbber'); })).toBe(true);
    await page.waitForFunction(() => window.__activityCalls.length === 1);
    expect(await page.evaluate(() => window.__activityCalls[0])).toMatchObject({ from_sequence: 2, through_sequence: 100 });
    await page.evaluate(() => window.__answerActivity(Array.from({ length: 99 }, (_, index) => index + 2)));
    await page.locator('.thread-activity-group [role="status"] .throbber').waitFor({ state: "detached" });
    expect(await page.locator(".thread-activity-group-list > .thread-activity").evaluateAll((rows) => rows.map((row) => Number(row.dataset.sequence))))
      .toEqual(Array.from({ length: 99 }, (_, index) => index + 2));
  }, { width: 390, height: 844 });
}, 30_000);

it("keeps an updated warm live tail window-backed and paints its later tool result", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountLeanThread(page, basePath, { warm: true });
    await page.evaluate(() => window.__updatedLeanTail());
    await page.waitForFunction(() => document.querySelector(".thread-activity-count")?.textContent === "99");
    const head = page.locator(".thread-activity-group-head");
    expect(await head.evaluate((node) => { node.click(); return !!document.querySelector('[role="status"] .throbber'); })).toBe(false);
    await page.waitForFunction(() => document.querySelectorAll(".thread-activity-group-list > .thread-activity").length === 2);
    expect(await page.locator(".thread-activity-group-list > .thread-activity").evaluateAll((rows) => rows.map((row) => row.dataset.sequence))).toEqual(["2", "100"]);
    expect(await page.evaluate(() => window.__activityCalls.length)).toBe(0);
    await page.evaluate(() => window.__updatedLeanTail(true));
    await page.waitForFunction(() => document.querySelector('.thread-activity-group-list [data-sequence="100"] .thread-activity-outcome')?.dataset.outcome === "ok");
    expect(await page.locator('.thread-activity-group-list [data-sequence="100"]').textContent()).toContain("Tool finished");
    expect(await page.evaluate(() => window.__activityCalls.length)).toBe(0);
  }, { width: 390, height: 844 });
}, 30_000);

it("retries a failed lean run inline in Chromium", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountLeanThread(page, basePath);
    await page.locator(".thread-activity-group-head").click();
    await page.waitForFunction(() => window.__pendingActivity.length === 1);
    await page.evaluate(() => window.__refuseActivity());
    const retry = page.locator("[data-activity-retry]");
    await retry.waitFor();
    expect(await page.locator('.thread-activity-group [role="alert"]').textContent()).toContain("Could not load this activity");
    await retry.click();
    await page.waitForFunction(() => window.__pendingActivity.length === 1);
    expect(await page.locator('.thread-activity-group [role="status"] .throbber').isVisible()).toBe(true);
    await page.evaluate(() => window.__answerActivity([2, 100]));
    await retry.waitFor({ state: "detached" });
    await page.locator('.thread-activity-group [role="status"] .throbber').waitFor({ state: "detached" });
    expect(await page.locator(".thread-activity-group-list > .thread-activity").count()).toBe(2);
  });
}, 30_000);
