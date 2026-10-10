import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

it("lands delayed New at 12px before reporting and preserves reader control", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div class="rail-body" id="scroller"></div>', {
      basePath,
      styles: 'body{display:block;height:auto} #scroller{width:640px;height:300px;margin:20px;overflow:auto} .thread-message{min-height:160px}',
    });
    await loadBrowserModules(page, { thread: "src/core/thread.js", landing: "src/core/chatLanding.js" }, basePath);
    const result = await page.evaluate(async () => {
      const { threadHtml, paintThreadKeepingPlace, readThroughSequence } = window.__layoutModules.thread;
      const { createChatLanding } = window.__layoutModules.landing;
      const body = document.querySelector("#scroller");
      const reports = [];
      const landing = createChatLanding(() => reports.push(readThroughSequence(body)));
      const thread = { items: Array.from({ length: 30 }, (_, i) => ({
        type: "message", data: { sequence: i + 1, role: "agent", body: `Message ${i + 1}` },
      })) };
      const paint = (target, waitingForHistory) => {
        const options = landing.prepare(body, { target, hasItems: true, waitingForHistory });
        paintThreadKeepingPlace(body, () => { body.innerHTML = threadHtml(thread, { unreadFrom: target }); }, { ...options, fallbackToStart: waitingForHistory });
        landing.painted();
      };
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      paint(null, true); // cached transcript precedes roster cursor
      await frame();
      const beforeRoster = reports.length;
      paint(12, false);
      const beforeFrame = reports.length;
      await frame();
      const lineOffset = body.querySelector(".thread-unread-line").getBoundingClientRect().top - body.getBoundingClientRect().top;
      const landedReports = [...reports];
      // A second opening followed by a reader gesture cancels the correction
      // queued for its frame, and also prevents the later divider chasing them.
      landing.reset();
      paint(null, true);
      body.dispatchEvent(new WheelEvent("wheel"));
      body.scrollTop = 80;
      body.dispatchEvent(new Event("scroll"));
      await frame();
      paint(12, false);
      const readerTop = body.scrollTop;
      landing.dispose();
      return { beforeRoster, beforeFrame, lineOffset, landedReports, readerTop };
    });
    expect(result.beforeRoster).toBe(0);
    expect(result.beforeFrame).toBe(0);
    expect(result.lineOffset).toBeCloseTo(12, 0);
    expect(result.landedReports.length).toBeGreaterThan(0);
    expect(result.landedReports.every((sequence) => sequence < 14)).toBe(true);
    expect(result.readerTop).toBe(80);
  });
}, 30_000);

it("keeps a missing-cursor opening at the cached start and reports only visible messages on timeout", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div class="rail-body" id="scroller"></div>', {
      basePath,
      styles: 'body{display:block;height:auto} #scroller{width:640px;height:300px;margin:20px;overflow:auto} .thread-message{min-height:160px}',
    });
    await loadBrowserModules(page, { thread: "src/core/thread.js", landing: "src/core/chatLanding.js" }, basePath);
    const result = await page.evaluate(async () => {
      const { threadHtml, paintThreadKeepingPlace, readThroughSequence } = window.__layoutModules.thread;
      const { createChatLanding, CHAT_LANDING_READ_WAIT_MS } = window.__layoutModules.landing;
      const body = document.querySelector("#scroller");
      const reports = [];
      const landing = createChatLanding(() => reports.push(readThroughSequence(body)));
      const thread = { items: Array.from({ length: 30 }, (_, i) => ({
        type: "message", data: { sequence: i + 1, role: "agent", body: `Message ${i + 1}` },
      })) };
      const options = landing.prepare(body, { target: null, hasItems: true, waitingForHistory: true });
      paintThreadKeepingPlace(body, () => { body.innerHTML = threadHtml(thread); }, { ...options, fallbackToStart: true });
      landing.painted();
      await new Promise(requestAnimationFrame);
      const openingTop = body.scrollTop;
      const viewport = body.getBoundingClientRect();
      const fullyVisible = [...body.querySelectorAll(".thread-message")].filter((row) => {
        const box = row.getBoundingClientRect();
        return box.top >= viewport.top && box.bottom <= viewport.bottom;
      }).map((row) => Number(row.dataset.sequence));
      const earlyReports = [...reports];
      await new Promise((done) => setTimeout(done, CHAT_LANDING_READ_WAIT_MS + 100));
      landing.dispose();
      return { openingTop, fullyVisible, earlyReports, reports };
    });
    expect(result.openingTop).toBe(0);
    expect(result.fullyVisible).toEqual([1]);
    expect(result.earlyReports).toEqual([]);
    expect(result.reports).toEqual([1]);
  });
}, 30_000);
