import { expect, it } from "vitest";
import answer from "../../../fixtures/api/v1/issues.get.json";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// The page needs no device connection for this cache-driven check. Keep every
// issue component real and replace only the connection recovery subscription.
const deviceShim = {
  name: "issue-identity-device-recovery-shim",
  enforce: "pre",
  resolveId(source, importer) {
    return source === "./deviceReconnect.js" && importer?.includes("/src/core/")
      ? "\0issue-identity-device-recovery" : null;
  },
  load(id) {
    if (id !== "\0issue-identity-device-recovery") return null;
    return `export const onDeviceMoved=()=>()=>{};
      export const deviceIsReconnecting=()=>false;
      export const deviceIsAway=()=>false;
      export const onDeviceReachable=()=>()=>{};
      export const deviceWatch=()=>({away:()=>false,reconnecting:()=>false,moved:()=>()=>{}});`;
  },
};

it("opens each unwatched identity from mounted issue, list, board and notice, then removes deleted routes", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await mountLayout(page, '<main id="issue"></main><main id="list"></main><main id="board"></main><main id="notice"></main>', {
      basePath,
      styles: `@import url("${basePath}src/styles/issues.css"); body{display:block} main{margin:12px}`,
    });
    await loadBrowserModules(page, {
      cache: "src/core/trackerCache.js",
      issuePage: "src/core/trackerIssuePage.js",
      issuesPane: "src/core/trackerIssuesPane.js",
      notice: "src/core/trackerNotice.js",
    }, basePath);
    await page.evaluate(async (fixture) => {
      const { cache, issuePage, issuesPane, notice } = window.__layoutModules;
      const agentId = "agent-01K5ZQ8M4T0J7WQ2R6X3YB9C4E";
      const workspaceId = "ws-3f2a91c4";
      const actor = { kind: "agent", agent_id: agentId };
      const issue = structuredClone(fixture.result.issue);
      issue.id = "issue-mounted";
      issue.body = `Ask @agent:${agentId} for the details.`;
      issue.assignee = actor;
      issue.links.workspace_ids = [workspaceId];
      const timeline = [
        { type: "comment", id: "ic-mounted", author: actor, body: "On it.", created_at: "2026-09-23T19:00:00Z" },
        { type: "event", id: "ie-mounted", kind: "assigned", actor: { kind: "user" }, payload: { assignee: actor }, at: "2026-09-23T19:01:00Z" },
        { type: "event", id: "ie-dispatched", kind: "dispatched", actor: { kind: "user" }, payload: { agent_id: agentId }, at: "2026-09-23T19:02:00Z" },
      ];
      const feed = { projects: [{ id: "proj-1", name: "Build", projectKey: "dev-1|proj-1" }],
        workspaces: [{ id: workspaceId, workspace_id: workspaceId, name: "spa-flaky-tests",
          projectKey: "dev-1|proj-1", entity_id: "run-unwatched" }], items: [] };
      const common = { deviceId: "dev-1", projectId: "proj-1", projectKey: "dev-1|proj-1",
        feed: () => feed, callRpc: async (method) => method === "issues.get" ? { issue, timeline }
          : method === "issues.list" ? { issues: [issue], columns: [] } : {},
        catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {} };
      await cache.writeIssueRecord("dev-1", "proj-1", issue.id, cache.issueRecord(issue, timeline));
      await cache.writeIssuesRecord("dev-1", "proj-1", { issues: [issue], columns: [] });
      const page = issuePage.mountIssuePage(document.querySelector("#issue"), { ...common, issueId: issue.id });
      const list = issuesPane.mountIssuesPane(document.querySelector("#list"), { ...common, view: "list" });
      const board = issuesPane.mountIssuesPane(document.querySelector("#board"), { ...common, view: "board" });
      const renderNotice = () => {
        document.querySelector("#notice").innerHTML = notice.issueNoticeLineHtml({
          issue_id: issue.id, number: issue.number, action: "assigned",
          actor: { ...actor, identity: issue.identities[agentId] },
          assignee: actor, assignee_identity: issue.identities[agentId],
        }, { place: common, projectName: "Build", workspaces: feed.workspaces });
      };
      renderNotice();
      window.__identityMounted = { feed, page, list, board, renderNotice };
    }, answer);

    await page.waitForFunction(() => document.querySelector("#issue .issue-comment .issue-entry-head a[href*='agent=']") &&
      document.querySelector("#list .issue-assignee-link[href*='agent=']") &&
      document.querySelector("#board .issue-assignee-link[href*='agent=']"));
    for (const selector of [
      "#issue .issue-comment .issue-entry-head a", "#issue .issue-page-body a",
      "#issue .issue-assignee-current a", "#issue .issue-event a[href*='agent=']",
      "#list .issue-assignee-link", "#board .issue-assignee-link",
      "#notice .thread-issue-by a", "#notice .thread-issue-said a",
    ]) expect(await page.locator(selector).first().getAttribute("href"), selector).toContain("agent=");
    expect(await page.locator("#notice a.thread-issue-number").getAttribute("href")).toContain("/issues/issue-mounted");
    expect(await page.locator("#list .issue-assign[data-issue-assign]").count()).toBe(1);
    expect(await page.locator("#board .issue-assign[data-issue-assign]").count()).toBe(1);

    await page.evaluate(() => {
      const mounted = window.__identityMounted;
      mounted.feed.workspaces = [];
      mounted.page.feedMoved();
      mounted.list.feedMoved();
      mounted.board.feedMoved();
      mounted.renderNotice();
    });
    await page.waitForFunction(() => !document.querySelector("#issue .issue-comment .issue-entry-head a") &&
      !document.querySelector("#issue .issue-page-body a") &&
      !document.querySelector("#issue .issue-assignee-current a") &&
      !document.querySelector("#list .issue-assignee-link") &&
      !document.querySelector("#board .issue-assignee-link") &&
      !document.querySelector("#notice a[href*='agent=']"));
    expect(await page.locator("#issue .issue-comment .issue-entry-head").textContent()).toContain("spa-flaky-tests · Fix drag");
    expect(await page.locator("#notice a.thread-issue-number").getAttribute("href")).toContain("/issues/issue-mounted");
    expect(errors).toEqual([]);
    await page.evaluate(() => {
      window.__identityMounted.page.dispose();
      window.__identityMounted.list.dispose();
      window.__identityMounted.board.dispose();
    });
  }, { plugins: [deviceShim] });
}, 30_000);
