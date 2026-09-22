import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

it("the grouped ticks consume only the chat's existing left padding", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div class="rail-body" id="scroller"></div>', {
      basePath,
      styles: 'body{display:block;height:auto} #scroller{box-sizing:border-box;width:640px;height:540px;margin:20px auto;overflow:auto}',
    });
    await loadBrowserModules(page, { thread: "src/core/thread.js" }, basePath);
    const measurements = await page.evaluate(() => {
      const { threadHtml } = window.__layoutModules.thread;
      const scroller = document.querySelector("#scroller");
      scroller.innerHTML = threadHtml({ id: "layout-check", items: [
        { type: "message", data: { sequence: 1, role: "user", body: "First user message", created_at: "2026-09-22T12:00:00Z" } },
        { type: "message", data: { sequence: 2, role: "agent", body: "An agent reply" } },
        { type: "message", data: { sequence: 3, role: "user", body: "Second user message", created_at: "2026-09-22T13:00:00Z" } },
      ] });
      const nav = scroller.querySelector(".thread-user-nav");
      const timeline = scroller.querySelector(".thread-items");
      const message = scroller.querySelector(".thread-message.user .thread-comment-card");
      const geometry = () => {
        const container = scroller.getBoundingClientRect();
        const column = timeline.getBoundingClientRect();
        const bubble = message.getBoundingClientRect();
        return {
          containerX: container.x,
          containerWidth: container.width,
          columnX: column.x,
          columnWidth: column.width,
          messageX: bubble.x,
          messageWidth: bubble.width,
        };
      };
      const present = geometry();
      nav.remove();
      const absent = geometry();
      timeline.before(nav);
      const scrollerBox = scroller.getBoundingClientRect();
      const pill = nav.querySelector(".thread-user-tick span").getBoundingClientRect();
      const tick = nav.querySelector(".thread-user-tick").getBoundingClientRect();
      return {
        present, absent,
        gutterLeft: scrollerBox.left,
        gutterRight: present.columnX,
        pillLeft: pill.left,
        pillRight: pill.right,
        tickLeft: tick.left,
        tickRight: tick.right,
      };
    });
    expect(measurements.present.columnWidth).toBeGreaterThan(0);
    expect(measurements.present).toEqual(measurements.absent);
    expect(measurements.gutterRight - measurements.gutterLeft).toBe(20);
    expect(measurements.pillLeft).toBeGreaterThanOrEqual(measurements.gutterLeft);
    expect(measurements.pillRight).toBeLessThanOrEqual(measurements.gutterRight);
  });
}, 30_000);
