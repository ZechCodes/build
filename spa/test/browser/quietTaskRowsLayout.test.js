import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

for (const phone of [false, true]) {
  it(`keeps quiet task titles above their facts on ${phone ? "a coarse phone" : "desktop"} (#501)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, '<div id="inbox-list" style="width:300px"></div>', { basePath });
      await loadBrowserModules(page, { rows: "src/core/inbox.js" }, basePath);
      await page.evaluate(() => {
        const { inboxRowHtml, TRACKER_TASK } = window.__layoutModules.rows;
        const task = { key: "tracker_task:assigned", kind: TRACKER_TASK, watched: true,
          name: "#34 Keep a long watched task title readable beside its assignment reason on a phone",
          facts: "Assigned to you", route: { name: "trackerTask" } };
        const paint = (entry, quiet) => inboxRowHtml(entry, { quiet, showProject: false });
        document.querySelector("#inbox-list").innerHTML = [
          paint(task, true), paint({ ...task, key: "tracker_task:live" }, false),
          paint({ ...task, key: "tracker_task:review", facts: "Mentioned you · In review",
            reviewSummary: { status: "open" } }, true),
          paint({ ...task, key: "tracker_task:bare", facts: "" }, true),
          paint({ ...task, key: "workspace:quiet", kind: "workspace", facts: "2 unpushed" }, true),
        ].join("");
      });
      expect(await page.evaluate(() => matchMedia("(pointer:coarse)").matches)).toBe(phone);
      const measurements = await page.locator(".inbox-entry").evaluateAll((rows) => rows.map((row) => {
        const title = row.querySelector(".stitle");
        const facts = row.querySelector(".inbox-facts");
        const box = (element) => element?.getBoundingClientRect().toJSON();
        return { title: box(title), facts: box(facts), height: row.getBoundingClientRect().height,
          text: title.textContent, ellipsis: getComputedStyle(title).textOverflow,
          clipped: title.scrollWidth > title.clientWidth,
          float: Boolean(row.querySelector(".inbox-facts-float")),
          watch: box(row.querySelector(".inbox-watch")),
          warnHidden: getComputedStyle(row.querySelector(".warn")).display === "none" };
      }));
      const [quiet, live, review, bare, workspace] = measurements;
      for (const row of [quiet, review]) {
        expect(row.text).toContain("#34 Keep a long watched task title");
        expect(row.title.width).toBeGreaterThan(100);
        expect(row.clipped).toBe(true);
        expect(row.ellipsis).toBe("ellipsis");
        expect(row.float).toBe(false);
        expect(row.facts.top).toBeGreaterThanOrEqual(row.title.bottom);
        expect(row.title.right).toBeLessThanOrEqual(row.watch.left);
        expect(row.warnHidden).toBe(true);
      }
      expect(quiet.height).toBe(live.height);
      expect(bare.facts).toBeUndefined();
      expect(workspace.float).toBe(true);
      if (phone) {
        expect(quiet.watch.height).toBe(44);
        // Workspace quiet rows keep their existing single-line padding/height.
        expect(workspace.height).toBeLessThan(36);
      }
    }, { width: phone ? 390 : 1280, height: 844, hasTouch: phone });
  });
}
