import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const task = {
  id: "layout-task", number: 341, title: "Tidy the task side panel",
  body: Array.from({ length: 24 }, (_, index) => `Section ${index + 1}. The task body keeps going while the details stay handy.`).join("\n\n"),
  state: "open", status: "in_progress", labels: ["ui"], priority: "medium", assignee: null,
  attachments: [], updated_at: "2026-10-02T20:00:00Z",
};

async function mountTask(page, basePath, { linkCount = 0, topInset = 0 } = {}) {
  await mountLayout(page, '<main class="task-surface" id="task-surface"></main>', {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css");
      body{display:block;margin:0;padding-top:${topInset}px}
      #task-surface{width:100vw;height:calc(100vh - ${topInset}px)}`,
  });
  await loadBrowserModules(page, { taskRender: "src/core/trackerTaskRender.js" }, basePath);
  await page.evaluate(({ record, linkCount: count }) => {
    document.querySelector("#task-surface").innerHTML = window.__layoutModules.taskRender.taskPageHtml(record, {
      rows: [], columns: [{ id: "in_progress", name: "In progress" }],
      links: Array.from({ length: count }, (_, index) => ({ label: `branch-${index + 1}` })),
      labelsDraft: "ui", busy: false, draft: "", sending: false, hasFiles: false,
    });
  }, { record: task, linkCount });
}

const measure = (scrollTop) => {
  const surface = document.querySelector("#task-surface");
  surface.scrollTop = scrollTop;
  const rail = document.querySelector(".task-rail");
  const main = document.querySelector(".task-page-main");
  return {
    scrollTop: surface.scrollTop,
    railTop: rail.getBoundingClientRect().top,
    railBottom: rail.getBoundingClientRect().bottom,
    mainBottom: main.getBoundingClientRect().bottom,
    surfaceBottom: surface.getBoundingClientRect().bottom,
    position: getComputedStyle(rail).position,
  };
};

it("keeps the task rail in view while the desktop task body scrolls", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountTask(page, basePath);
    const before = await page.evaluate(measure, 0);
    const after = await page.evaluate(measure, 300);
    expect(after.scrollTop).toBe(300);
    expect(after.position).toBe("sticky");
    expect(Math.abs(after.railTop - before.railTop)).toBeLessThan(2);
    expect(after.railBottom).toBeLessThanOrEqual(after.surfaceBottom);
    await captureLayout(page, "task-rail-desktop-after.png");
  }, { width: 1280, height: 800 });
}, 30_000);

it("keeps Close task reachable by scrolling an overflowing rail in a short desktop window", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountTask(page, basePath, { linkCount: 24, topInset: 80 });
    const before = await page.evaluate(() => {
      const surface = document.querySelector("#task-surface");
      const rail = document.querySelector(".task-rail");
      surface.scrollTop = 250;
      return { surfaceScrollTop: surface.scrollTop, railHeight: rail.clientHeight, contentHeight: rail.scrollHeight };
    });
    expect(before.surfaceScrollTop).toBe(250);
    expect(before.contentHeight).toBeGreaterThan(before.railHeight);

    const after = await page.evaluate(() => {
      const surface = document.querySelector("#task-surface");
      const rail = document.querySelector(".task-rail");
      rail.scrollTop = rail.scrollHeight;
      const close = rail.querySelector("[data-task-state]").getBoundingClientRect();
      const bounds = rail.getBoundingClientRect();
      return {
        railScrollTop: rail.scrollTop, surfaceScrollTop: surface.scrollTop,
        railTop: bounds.top, railBottom: bounds.bottom,
        surfaceBottom: surface.getBoundingClientRect().bottom,
        closeTop: close.top, closeBottom: close.bottom,
      };
    });
    expect(after.railScrollTop).toBeGreaterThan(0);
    expect(after.surfaceScrollTop).toBe(250);
    expect(after.closeTop).toBeGreaterThanOrEqual(after.railTop);
    expect(after.closeBottom).toBeLessThanOrEqual(after.railBottom);
    expect(after.railBottom).toBeLessThan(after.surfaceBottom);
    await captureLayout(page, "task-rail-short-desktop-after.png");
  }, { width: 1280, height: 500 });
}, 30_000);

it("leaves the stacked task rail in the mobile page flow", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountTask(page, basePath, { linkCount: 24 });
    const before = await page.evaluate(measure, 0);
    const after = await page.evaluate(measure, 300);
    expect(before.railTop).toBeGreaterThanOrEqual(before.mainBottom);
    expect(after.scrollTop).toBe(300);
    expect(after.position).toBe("static");
    expect(before.railTop - after.railTop).toBeGreaterThan(298);
    expect(await page.locator(".task-rail").evaluate((rail) => getComputedStyle(rail).overflowY)).toBe("visible");
    await page.evaluate(() => {
      const surface = document.querySelector("#task-surface");
      surface.scrollTop = surface.scrollHeight;
    });
    await captureLayout(page, "task-rail-mobile-after.png");
  }, { width: 390, height: 800 });
}, 30_000);
