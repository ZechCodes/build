import { readFile } from "node:fs/promises";
import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./issueIdentityHarness.mjs";

// The normal suite replays a trace exported by the Rust AppState regression.
// The dedicated cross-stack check sets this path to a freshly generated trace,
// exercising the real agent.remove handler and subscription flush as well.
const tracePath = process.env.BUILD_ISSUE_AGENT_REMOVAL_TRACE || new URL("../fixtures/issueAgentRemoval.json", import.meta.url);

it("reconciles a real agent removal into mounted issue, list and board identities while the workspace remains", async () => {
  const trace = JSON.parse(await readFile(tracePath, "utf8"));
  const issue = trace.before.get.issue;
  const removed = issue.assignee.agent_id;
  const retained = Object.keys(issue.identities).find((id) => id !== removed && issue.identities[id].available);
  expect(retained, "the real trace also contains a live agent with no watched roster").toBeTruthy();
  const identity = issue.identities[removed];
  const actorName = `${identity.workspace_name} · ${identity.name}`;
  const linkFor = (id) => `a[href*="agent=${id}"]`;
  const removedLink = linkFor(removed);
  const surfaces = [
    "#issue .issue-comment .issue-entry-head", "#issue .issue-page-body", "#issue .issue-assignee-current",
    `#list [data-issue="${issue.id}"]`, `#board [data-issue="${issue.id}"]`,
  ];

  await withLayoutPage(async ({ page, basePath }) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await mountLayout(page, '<main id="issue"></main><main id="list"></main><main id="board"></main>', {
      basePath, styles: `@import url("${basePath}src/styles/issues.css"); body{display:block} main{margin:12px}`,
    });
    await loadBrowserModules(page, {
      cache: "src/core/trackerCache.js", changes: "src/core/changeEvents.js",
      issuePage: "src/core/trackerIssuePage.js", issuesPane: "src/core/trackerIssuesPane.js",
    }, basePath);
    await page.evaluate(async (trace) => {
      const { cache, changes, issuePage, issuesPane } = window.__layoutModules;
      const issue = trace.before.get.issue;
      const identity = issue.identities[issue.assignee.agent_id];
      const deviceId = "identity-removal-device";
      const projectId = issue.project_id;
      const projectKey = `${deviceId}|${projectId}`;
      const feed = {
        projects: [{ id: projectId, name: "Build", projectKey }],
        workspaces: [{ id: identity.workspace_id, workspace_id: identity.workspace_id, name: identity.workspace_name, projectKey }],
        items: [], // These agents are unwatched; missing roster data is not deletion.
      };
      const replay = { phase: "before", calls: [], trace, deviceId, projectId, issueId: issue.id, feed };
      const callRpc = async (method, params) => {
        replay.calls.push({ method, params, phase: replay.phase });
        if (method === "session.hello") return trace.greeting;
        if (method === "issues.get" && params.issue_id === issue.id) return structuredClone(trace[replay.phase].get);
        if (method === "issues.list") return structuredClone(trace[replay.phase].list);
        return {};
      };
      await changes.greetBridge(callRpc, { deviceId });
      await cache.writeIssueRecord(deviceId, projectId, issue.id, cache.issueRecord(issue, trace.before.get.timeline));
      await cache.writeIssuesRecord(deviceId, projectId, trace.before.list);
      const common = { deviceId, projectId, projectKey, feed: () => feed, callRpc,
        catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {} };
      replay.page = issuePage.mountIssuePage(document.querySelector("#issue"), { ...common, issueId: issue.id });
      replay.list = issuesPane.mountIssuesPane(document.querySelector("#list"), { ...common, view: "list" });
      replay.board = issuesPane.mountIssuesPane(document.querySelector("#board"), { ...common, view: "board" });
      window.__removalReplay = replay;
      await changes.subscriptionsSettled();
    }, trace);

    for (const surface of surfaces) {
      await page.locator(`${surface} ${removedLink}`).first().waitFor();
    }
    expect(await page.locator(`#issue .issue-page-body ${linkFor(retained)}`).count()).toBe(1);
    expect(await page.evaluate(() => window.__removalReplay.feed.items)).toEqual([]);
    expect(await page.evaluate(() => window.__layoutModules.changes.bridgeCapabilities("identity-removal-device").changes.kinds)).toContain("issues");

    // Only replay the actual bridge flush. The production subscriptions pull
    // new RPC results, write IndexedDB, and repaint on cache announcements.
    // No view refresh, feedMoved, or cache write is called from this test.
    await page.evaluate(async () => {
      const { changes } = window.__layoutModules;
      const replay = window.__removalReplay;
      await changes.subscriptionsSettled();
      replay.phase = "after";
      for (const event of replay.trace.events) changes.dispatchChangeEvent(event, replay.deviceId);
    });
    await expect.poll(() => page.evaluate(
      (selectors) => selectors.map((selector) => document.querySelectorAll(selector).length),
      surfaces.map((surface) => `${surface} ${removedLink}`),
    ), { timeout: 5_000 }).toEqual(surfaces.map(() => 0));
    expect(await page.locator(removedLink).count()).toBe(0);
    for (const selector of ["#issue .issue-comment", `#list [data-issue="${issue.id}"]`, `#board [data-issue="${issue.id}"]`]) {
      expect(await page.locator(selector).textContent()).toContain(actorName);
      expect(await page.locator(selector).locator(`[data-harness-icon="${identity.provider}"]`).count()).toBeGreaterThan(0);
    }
    expect(await page.locator(`#issue .issue-page-body ${linkFor(retained)}`).count()).toBe(1);
    expect(await page.evaluate(() => window.__removalReplay.feed.workspaces.length)).toBe(1);

    const reconciled = await page.evaluate(async (removed) => {
      const { cache } = window.__layoutModules;
      const { deviceId, projectId, issueId, calls } = window.__removalReplay;
      const detail = await cache.readIssueRecord(deviceId, projectId, issueId);
      const list = await cache.readIssuesQueryRecord(deviceId, projectId, { project_id: projectId, state: "open" });
      return {
        detail: detail.issue.identities[removed],
        list: list.issues.find((issue) => issue.id === issueId).identities[removed],
        pulled: calls.filter((call) => call.phase === "after").map((call) => call.method),
      };
    }, removed);
    expect(reconciled.detail).toMatchObject({ name: identity.name, provider: identity.provider, available: false });
    expect(reconciled.list).toMatchObject({ name: identity.name, provider: identity.provider, available: false });
    expect(reconciled.pulled).toContain("issues.get");
    expect(reconciled.pulled).toContain("issues.list");
    for (const view of ["list", "board"]) {
      await page.locator(`#${view} [data-issue="${issue.id}"] [data-issue-assign]`).click();
      const picker = page.locator("#issue-assign-scrim [role='dialog']");
      await picker.waitFor({ state: "visible", timeout: 5_000 });
      await picker.locator("[data-assign-cancel]").click();
      await picker.waitFor({ state: "detached" });
    }
    expect(errors).toEqual([]);
    await page.evaluate(() => {
      window.__removalReplay.page.dispose();
      window.__removalReplay.list.dispose();
      window.__removalReplay.board.dispose();
      window.__layoutModules.changes.resetChangeEvents();
    });
  }, { plugins: [deviceShim] });
}, 30_000);
