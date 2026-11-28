import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { expectNewHeadroom } from "./chatLandingAssertions.mjs";

it("lands delayed New at 12px before reporting and preserves reader control", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div class="rail-panel"><div class="rail-head">Conversation</div><div class="rail-body" id="scroller"></div></div>', {
      basePath,
      styles: 'body{display:block;height:auto} .rail-panel{position:relative;inset:auto;width:640px;height:300px;margin:20px} .thread-message{min-height:160px}',
    });
    await loadBrowserModules(page, { thread: "src/core/thread.js", landing: "src/core/openingPosition.js" }, basePath);
    const result = await page.evaluate(async () => {
      const { threadHtml, paintThreadKeepingPlace, readThroughSequence } = window.__layoutModules.thread;
      const { createOpeningPosition } = window.__layoutModules.landing;
      const body = document.querySelector("#scroller");
      const reports = [];
      const landing = createOpeningPosition(() => reports.push(readThroughSequence(body)));
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
      const lineOffset = body.querySelector(".thread-unread-line").getBoundingClientRect().top
        - body.parentElement.querySelector(".rail-head").getBoundingClientRect().bottom;
      const landedReports = [...reports];
      // A usage banner moves the body below the head and zeroes scroll padding.
      // The frame correction must read that new clearance, not the first paint's.
      landing.reset();
      paint(12, false);
      const banner = document.createElement("div");
      banner.className = "usage-limit-banners";
      banner.textContent = "Usage limit";
      body.before(banner);
      await frame();
      const bannerLineOffset = body.querySelector(".thread-unread-line").getBoundingClientRect().top
        - body.getBoundingClientRect().top;
      banner.remove();
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
      return { beforeRoster, beforeFrame, lineOffset, bannerLineOffset, landedReports, readerTop };
    });
    expect(result.beforeRoster).toBe(0);
    expect(result.beforeFrame).toBe(0);
    expectNewHeadroom(result.lineOffset);
    expectNewHeadroom(result.bannerLineOffset);
    expect(result.landedReports.length).toBeGreaterThan(0);
    expect(result.landedReports.every((sequence) => sequence < 14)).toBe(true);
    expect(result.readerTop).toBe(80);
  });
}, 30_000);

it.each(["Enter", "Space"])("keeps keyboard-expanded activity in view after %s", async (key) => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div class="rail-body" id="rail-body"></div>', {
      basePath,
      styles: 'body{display:block;height:auto} #rail-body{width:640px;height:300px;margin:20px;overflow:auto;padding:0} .thread-message:first-child{min-height:650px} .thread-message:last-child{min-height:160px}',
    });
    await loadBrowserModules(page, {
      thread: "src/core/thread.js", landing: "src/core/openingPosition.js", reveal: "src/core/revealExpanded.js",
    }, basePath);
    const bottomGap = await page.evaluate(async () => {
      const { threadHtml, paintThreadKeepingPlace } = window.__layoutModules.thread;
      const { createOpeningPosition } = window.__layoutModules.landing;
      const { wireExpansionReveal } = window.__layoutModules.reveal;
      const body = document.querySelector("#rail-body");
      const landing = createOpeningPosition(() => {});
      const options = landing.prepare(body, { target: null, hasItems: true, waitingForHistory: false });
      const thread = { items: [
        { type: "message", data: { sequence: 1, role: "agent", body: "Before the activity" } },
        { type: "event", data: { sequence: 2, event: "tool_use", summary: "Read report\n\n" + "More detail.\n\n".repeat(20) } },
        { type: "message", data: { sequence: 3, role: "agent", body: "Newest reply" } },
      ] };
      paintThreadKeepingPlace(body, () => {
        body.innerHTML = threadHtml(thread, { openRuns: new Set(["2"]) });
      }, options);
      wireExpansionReveal(body);
      landing.painted(body.querySelector(".review-thread"));
      await new Promise(requestAnimationFrame);
      // Keyboard focus must not scroll and release the anchor before activation.
      body.querySelector(".thread-activity > summary").focus({ preventScroll: true });
      return body.scrollHeight - body.clientHeight - body.scrollTop;
    });
    expect(bottomGap).toBeLessThanOrEqual(1);

    await page.keyboard.press(key);
    const summary = page.locator(".thread-activity[open] > summary");
    const position = await summary.evaluate(async (head) => {
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const scroller = head.closest("#rail-body");
      const viewport = scroller.getBoundingClientRect();
      const rect = head.getBoundingClientRect();
      return {
        bottomGap: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop,
        visible: rect.top >= viewport.top - 1 && rect.bottom <= viewport.bottom,
      };
    });
    expect(position.bottomGap).toBeGreaterThan(32);
    expect(position.visible).toBe(true);
  });
}, 30_000);

it("keeps a missing-cursor opening at the cached start and reports only visible messages on timeout", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div class="rail-body" id="scroller"></div>', {
      basePath,
      styles: 'body{display:block;height:auto} #scroller{width:640px;height:300px;margin:20px;overflow:auto} .thread-message{min-height:160px}',
    });
    await loadBrowserModules(page, { thread: "src/core/thread.js", landing: "src/core/openingPosition.js" }, basePath);
    const result = await page.evaluate(async () => {
      const { threadHtml, paintThreadKeepingPlace, readThroughSequence } = window.__layoutModules.thread;
      const { createOpeningPosition, OPENING_READ_WAIT_MS } = window.__layoutModules.landing;
      const body = document.querySelector("#scroller");
      const reports = [];
      const landing = createOpeningPosition(() => reports.push(readThroughSequence(body)));
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
      await new Promise((done) => setTimeout(done, OPENING_READ_WAIT_MS + 100));
      landing.dispose();
      return { openingTop, fullyVisible, earlyReports, reports };
    });
    expect(result.openingTop).toBe(0);
    expect(result.fullyVisible).toEqual([1]);
    expect(result.earlyReports).toEqual([]);
    expect(result.reports).toEqual([1]);
  });
}, 30_000);
