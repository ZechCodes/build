import { expect, it } from "vitest";
import { mountLayout, loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

const fixture = () => {
  const timeline = [{
    type: "event", id: "ie-0000", kind: "created", at: "2026-09-23T12:00:00Z",
    actor: { kind: "user" }, payload: {},
  }];
  for (let number = 1; number <= 24; number += 1) {
    timeline.push({
      type: "comment",
      id: `ic-${String(number).padStart(4, "0")}`,
      issue_id: "layout-issue",
      author: number % 3 ? { kind: "agent", id: "layout-agent" } : { kind: "user" },
      body: `Comment ${number}. ${"A short discussion of this issue. ".repeat(2)}`,
      created_at: `2026-09-23T12:${String(number).padStart(2, "0")}:00Z`,
    });
  }
  return {
    issue: {
      id: "layout-issue", project_id: "layout-project", number: 99,
      title: "Issue comments with unread activity", body: "A reader returns to this issue after several replies.",
      state: "open", status: "in_progress", labels: [], priority: "none", assignee: null,
      links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_issue_id: null },
      created_by: { kind: "user" }, created_at: "2026-09-23T12:00:00Z", updated_at: "2026-09-23T12:24:00Z",
      read_through: "ic-0016", watched: true, trackers: [],
    },
    timeline,
  };
};

for (const { label, width, height } of [
  { label: "desktop", width: 1440, height: 800 },
  { label: "mobile", width: 390, height: 760 },
]) {
  it(`shows the issue unread line and pill, then jumps on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, '<main class="issue-surface" id="issue-layout"></main>', {
        basePath,
        styles: `@import url("${basePath}src/styles/issues.css");
          body{display:block;margin:0;width:100vw;height:100vh}
          #issue-layout{height:100vh;width:100vw;max-width:none}`,
      });
      await loadBrowserModules(page, {
        issueRender: "src/core/trackerIssueRender.js",
        timeline: "src/core/trackerTimeline.js",
        unread: "src/core/trackerUnread.js",
        marker: "src/core/unreadAnchor.js",
        pill: "src/core/newMessagesPill.js",
      }, basePath);
      await page.evaluate((answer) => {
        const host = document.querySelector("#issue-layout");
        const { issueRender, timeline, unread, marker, pill } = window.__layoutModules;
        const rows = timeline.timelineRows(answer.timeline);
        const unreadFrom = marker.createUnreadMarker(() => {}, unread.issueUnreadRules)
          .update(unread.issueUnreadReading(rows, answer.issue.read_through));
        host.innerHTML = issueRender.issuePageHtml(answer.issue, {
          rows, unreadFrom, columns: [
            { id: "backlog", name: "Backlog" }, { id: "in_progress", name: "In progress" },
          ],
          agentLabels: {}, agentProviders: {}, projectName: "Build", refLinks: null,
          links: [], watch: null, draft: "", labelsDraft: "", busy: false,
          sending: false, attachable: false, hasFiles: false,
        });
        const beforeHeight = host.scrollHeight;
        window.__layoutPill = pill.mountNewMessagesPill(host, { targetSelector: ".issue-unread-line" });
        window.__layoutPill.sync();
        window.__layoutHeights = { before: beforeHeight, after: host.scrollHeight };
      }, fixture());

      await page.waitForSelector(".issue-comment", { state: "visible" });
      await page.waitForSelector(".thread-unread-line", { state: "attached" });
      await page.waitForSelector(".new-messages-pill", { state: "visible" });
      expect(await page.locator(".issue-comment").count()).toBe(24);
      const overlay = await page.evaluate(() => {
        const host = document.querySelector("#issue-layout").getBoundingClientRect();
        const pill = document.querySelector(".new-messages-pill").getBoundingClientRect();
        const dock = document.querySelector(".new-messages-dock").getBoundingClientRect();
        return {
          heights: window.__layoutHeights, hostBottom: host.bottom,
          pillBottom: pill.bottom, pillHeight: pill.height, dockHeight: dock.height,
        };
      });
      expect(overlay.heights.after).toBe(overlay.heights.before);
      expect(overlay.dockHeight).toBe(0);
      expect(overlay.pillHeight).toBeGreaterThanOrEqual(30);
      expect(overlay.pillBottom).toBeLessThanOrEqual(overlay.hostBottom);
      expect(overlay.pillBottom).toBeGreaterThan(overlay.hostBottom - 100);
      await page.screenshot({ path: `/tmp/issue-unread-${label}-before.png` });

      await page.locator(".new-messages-pill").click();
      await page.waitForFunction(() => {
        const host = document.querySelector("#issue-layout");
        const firstUnread = document.querySelector("#comment-ic-0017");
        const pill = document.querySelector(".new-messages-pill");
        if (!host || !firstUnread || !pill) return false;
        const bounds = host.getBoundingClientRect();
        const row = firstUnread.getBoundingClientRect();
        return row.top >= bounds.top && row.top < bounds.bottom && !pill.getClientRects().length;
      });
      const after = await page.evaluate(() => {
        const host = document.querySelector("#issue-layout");
        const firstUnread = document.querySelector("#comment-ic-0017");
        const pill = document.querySelector(".new-messages-pill");
        const bounds = host.getBoundingClientRect();
        const row = firstUnread.getBoundingClientRect();
        return {
          firstUnreadTop: row.top, surfaceTop: bounds.top, surfaceBottom: bounds.bottom,
          pillVisible: Boolean(pill?.getClientRects().length), scrollTop: host.scrollTop,
        };
      });
      expect(after.firstUnreadTop, JSON.stringify(after)).toBeGreaterThanOrEqual(after.surfaceTop);
      expect(after.firstUnreadTop, JSON.stringify(after)).toBeLessThan(after.surfaceBottom);
      expect(after.pillVisible, JSON.stringify(after)).toBe(false);
      await page.screenshot({ path: `/tmp/issue-unread-${label}-after.png` });
      await page.evaluate(() => window.__layoutPill.dispose());
    }, { width, height });
  }, 30_000);
}
