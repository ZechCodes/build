import { expect, it } from "vitest";
import { captureLayout, withLayoutPage } from "./layoutHarness.mjs";
import { mountReclaimStatusDot, mountStatusDotInbox, mountWaitingStatusDotInbox } from "./inboxStatusDotSeed.mjs";

async function rowLayout(row) {
  return row.evaluate((element) => {
    const dot = element.querySelector(".inbox-status-dot");
    const rect = (node) => {
      const box = node.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, width: box.width, height: box.height };
    };
    const css = dot && getComputedStyle(dot);
    const actions = element.querySelector(".inbox-actions");
    const row = rect(element);
    const name = rect(element.querySelector(".stitle"));
    return {
      key: element.dataset.key, row, name, leftIcons: element.querySelectorAll(":scope > .sdot").length,
      counters: element.querySelectorAll(".inbox-unread").length,
      dot: dot && { ...rect(dot), background: css.backgroundColor, animation: css.animationName,
        opacity: css.opacity, green: getComputedStyle(element).getPropertyValue("--green").trim(),
        running: dot.classList.contains("inbox-status-running"), unread: dot.classList.contains("inbox-status-unread") },
      actionsOpacity: actions && getComputedStyle(actions).opacity,
      controls: [...(actions?.querySelectorAll(":scope > button") || [])].map((button) => ({
        ...rect(button), opacity: getComputedStyle(button).opacity, visibility: getComputedStyle(button).visibility,
      })),
    };
  });
}

for (const { label, width, height } of [
  { label: "desktop", width: 1280, height: 800 },
  { label: "mobile", width: 390, height: 760 },
]) {
  for (const theme of ["light", "dark"]) {
    it(`aligns row status dots in the left gutter on ${label} in ${theme}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await page.emulateMedia({ reducedMotion: "reduce" });
        await mountStatusDotInbox(page, basePath, { grouped: true });
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        const block = page.locator(".inbox-project");
        const head = block.locator(".inbox-project-head");
        expect(await head.locator(":scope > .inbox-status-dot").count()).toBe(0);
        const dot = block.locator('.inbox-project-agent .inbox-status-dot');
        const rows = block.locator(".inbox-entry:visible").filter({ has: page.locator(".inbox-status-dot") });
        expect(await block.locator('[data-key^="workspace:"] .inbox-status-dot').count()).toBeGreaterThan(0);
        expect(await block.locator('[data-key^="task:"] .inbox-status-dot').count()).toBe(1);
        const before = await dot.boundingBox();
        const nameBefore = await head.locator(".inbox-project-name").boundingBox();
        for (const active of [false, true]) {
          await block.evaluate((element, active) => { element.classList.toggle("active", active); }, active);
          await rows.first().evaluate((element, active) => { element.classList.toggle("active", active); }, active);
          for (const hovered of [false, true]) {
            if (hovered) {
              await head.hover();
              await page.waitForFunction(() => getComputedStyle(document.querySelector(".inbox-project-actions")).opacity === "1");
            } else await page.mouse.move(width - 1, height - 1);
            const state = `${active ? "active" : "inactive"}, ${hovered ? "head hovered" : "resting"}`;
            await captureLayout(page, `inbox-status-dot-alignment-${label}-${theme}-${active ? "active" : "inactive"}-${hovered ? "hovered" : "resting"}.png`);
            expect(await head.locator(":scope > .inbox-status-dot").count(), state).toBe(0);
            expect(await dot.boundingBox(), state).toEqual(before);
            expect(await head.locator(".inbox-project-name").boundingBox(), state).toEqual(nameBefore);
            const left = await dot.evaluate((element) => element.getBoundingClientRect().left);
            for (const row of await rows.all()) {
              const layout = await rowLayout(row);
              expect.soft(layout.dot.left, `${state}: ${layout.key}`).toBe(left);
              expect.soft(layout.dot.right, `${state}: ${layout.key}`).toBeLessThan(layout.name.left);
              expect.soft(layout.dot.top + 4, `${state}: ${layout.key}`).toBeCloseTo(layout.name.top + layout.name.height / 2, 1);
              if (!hovered) {
                await row.hover();
                expect.soft((await rowLayout(row)).dot.left, `${state}: ${layout.key} hovered`).toBe(left);
                await page.mouse.move(width - 1, height - 1);
              }
            }
            for (const button of await head.locator(".inbox-project-create, .inbox-project-hide").all()) {
              const box = await button.boundingBox();
              expect(box.x, state).toBeGreaterThanOrEqual(nameBefore.x + nameBefore.width);
            }
          }
        }
      }, { width, height });
    }, 60_000);
  }
  it(`pulses waiting-agent inbox rows and watched project summaries on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      for (const { ownWaiting, workspaceWatched, unreadCount } of [
        { ownWaiting: true, workspaceWatched: true, unreadCount: 0 },
        { ownWaiting: true, workspaceWatched: true, unreadCount: 2 },
        { ownWaiting: false, workspaceWatched: true, unreadCount: 0 },
        { ownWaiting: false, workspaceWatched: false, unreadCount: 0 },
      ]) {
        for (const { grouped, folded } of [{ grouped: false, folded: false },
          { grouped: true, folded: false }, { grouped: true, folded: true }]) {
          await mountWaitingStatusDotInbox(page, basePath, { grouped, folded, ownWaiting, workspaceWatched, unreadCount });
          const projectDot = page.locator(grouped && folded ? ".inbox-project-head > .inbox-status-dot"
            : '[data-key="project-agent:layout-device/layout-project"] .inbox-status-dot');
          if (grouped && !folded) expect(await page.locator(".inbox-project-head > .inbox-status-dot").count()).toBe(0);
          const projectRuns = ownWaiting || (folded && workspaceWatched);
          expect(await projectDot.count()).toBe(projectRuns || unreadCount > 0 ? 1 : 0);
          if (projectRuns || unreadCount > 0) {
            expect(await projectDot.evaluate((dot) => dot.classList.contains("inbox-status-running"))).toBe(projectRuns);
            expect(await projectDot.evaluate((dot) => dot.classList.contains("inbox-status-unread"))).toBe(unreadCount > 0);
            expect(await projectDot.evaluate((dot) => getComputedStyle(dot).animationName === "none")).toBe(!projectRuns);
          }
          if (folded) continue;
          const workspace = page.locator('[data-key="workspace:layout-device/waiting"]');
          expect(await workspace.locator(".inbox-status-running").count()).toBe(1);
          expect(await workspace.locator(".inbox-status-unread").count()).toBe(unreadCount > 0 && workspaceWatched ? 1 : 0);
          expect(await workspace.locator(".inbox-facts").textContent()).toContain("1 running");
        }
      }
    }, { width, height });
  }, 60_000);

  it(`keeps one left status dot clear of names and hover controls on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      for (const grouped of [false, true]) {
        await mountStatusDotInbox(page, basePath, { grouped });
        const entries = page.locator("#inbox-list .inbox-entry:visible");
        for (const row of await entries.all()) {
          const key = await row.getAttribute("data-key");
          const idle = key.endsWith("/read");
          const before = await rowLayout(row);
          expect(before.leftIcons, key).toBe(0);
          expect(before.counters, key).toBe(0);
          expect(await row.locator(".inbox-status-dot").count(), key).toBe(idle ? 0 : 1);
          if (idle) continue;
          expect(before.dot.width, key).toBe(8);
          expect(before.dot.height, key).toBe(8);
          expect(before.dot.left, key).toBeGreaterThanOrEqual(before.row.left + 6);
          expect(before.dot.right, key).toBeLessThan(before.name.left);
          expect(before.dot.top + 4, key).toBeCloseTo(before.name.top + before.name.height / 2, 1);
          expect(before.dot.animation === "none", key).toBe(!before.dot.running);
          await row.hover();
          await page.waitForFunction((key) => [...document.querySelector(`[data-key="${key}"] .inbox-actions`).children]
            .filter((child) => child.matches("button")).every((button) => getComputedStyle(button).opacity === "1"), key);
          const hovered = await rowLayout(row);
          expect(hovered.dot.left, key).toBe(before.dot.left);
          for (const control of hovered.controls) {
            expect(control.opacity, key).toBe("1");
            expect(control.visibility, key).toBe("visible");
            expect(control.left, key).toBeGreaterThanOrEqual(hovered.name.right);
          }
        }
        await captureLayout(page, `inbox-status-dot-${label}-${grouped ? "grouped" : "ungrouped"}.png`);
      }
    }, { width, height });
  }, 60_000);

  it(`shows project own status on its row expanded and aggregate status on its heading folded on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      for (const folded of [false, true]) {
        await mountStatusDotInbox(page, basePath, { grouped: true, folded });
        const head = page.locator(".inbox-project-head");
        expect(await head.locator(".inbox-unread, .inbox-more, .inbox-project-settings, .inbox-menu").count()).toBe(0);
        const dot = head.locator(":scope > .inbox-status-dot");
        expect(await dot.count()).toBe(folded ? 1 : 0);
        if (folded) expect(await dot.evaluate((element) => element === element.parentElement.lastElementChild)).toBe(true);
        const status = folded ? dot : page.locator('.inbox-project-agent .inbox-status-dot');
        expect(await status.evaluate((element) => element.classList.contains("inbox-status-running"))).toBe(folded);
        expect(await status.evaluate((element) => element.classList.contains("inbox-status-unread"))).toBe(true);
        const before = await status.boundingBox();
        await head.hover();
        await page.waitForFunction(() => getComputedStyle(document.querySelector(".inbox-project-actions")).opacity === "1");
        const after = await status.boundingBox();
        expect(after).toEqual(before);
        const name = await head.locator(".inbox-project-name").boundingBox();
        expect(after.x + after.width).toBeLessThan(name.x);
        if (folded) expect(after.y + after.height / 2).toBeCloseTo(name.y + name.height / 2, 1);
        const fold = await head.locator(".inbox-fold").boundingBox();
        if (folded) expect(after.x + after.width).toBeLessThanOrEqual(fold.x - 2);
        const actions = head.locator(".inbox-project-actions");
        expect(await actions.evaluate((element) => getComputedStyle(element).opacity)).toBe("1");
        for (const button of await head.locator(".inbox-project-create, .inbox-project-hide").all()) {
          const box = await button.boundingBox();
          expect(box.x).toBeGreaterThanOrEqual(name.x + name.width);
        }
        expect(await head.locator(".inbox-project-hide").count()).toBe(1);
      }
      await mountStatusDotInbox(page, basePath, { grouped: true, ownUnread: 0 });
      expect(await page.locator(".inbox-project-head > .inbox-status-dot").count()).toBe(0);
      expect(await page.locator(".inbox-project-agent .inbox-status-dot").count()).toBe(0);
      await mountStatusDotInbox(page, basePath, { grouped: true, folded: true, ownUnread: 0 });
      expect(await page.locator(".inbox-project-head > .inbox-status-dot.inbox-status-unread.inbox-status-running").count()).toBe(1);
    }, { width, height });
  }, 60_000);

  it(`keeps name space stable when row and project dots change state on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountStatusDotInbox(page, basePath);
      const row = page.locator('[data-key="workspace:layout-device/running"]');
      const boxes = [];
      for (const { working, unreadCount } of [
        { working: false, unreadCount: 0 }, { working: true, unreadCount: 0 },
        { working: false, unreadCount: 1 }, { working: true, unreadCount: 1 },
      ]) {
        await page.evaluate(({ working, unreadCount }) => {
          const entry = window.__statusDotEntries.find((entry) => entry.key.endsWith("/running"));
          Object.assign(entry, { working, unreadCount, state: working ? "working" : unreadCount ? "unread" : "inactive" });
          document.querySelector('[data-key="workspace:layout-device/running"]').outerHTML = window.__layoutModules.rows.inboxRowHtml(entry);
        }, { working, unreadCount });
        boxes.push(await row.locator(".stitle").boundingBox());
      }
      for (const box of boxes) expect.soft(box).toEqual(boxes[0]);
      const heads = [];
      for (const { ownUnread, ownWorking } of [
        { ownUnread: 0, ownWorking: false }, { ownUnread: 0, ownWorking: true },
        { ownUnread: 2, ownWorking: false }, { ownUnread: 2, ownWorking: true },
      ]) {
        await mountStatusDotInbox(page, basePath, { grouped: true, ownUnread, ownWorking });
        expect(await page.locator(".inbox-project-head > .inbox-status-dot").count()).toBe(0);
        heads.push(await page.locator(".inbox-project-name").boundingBox());
      }
      for (const box of heads) expect.soft(box).toEqual(heads[0]);
    }, { width, height });
  }, 60_000);

  it(`keeps a project workspace name clear of Reclaim and Reclaiming… on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountReclaimStatusDot(page, basePath);
      const row = page.locator(".project-row");
      await row.hover();
      await page.waitForFunction(() => getComputedStyle(document.querySelector("[data-workspace-reclaim]")).opacity === "1");
      const before = await rowLayout(row);
      const reclaim = row.locator("[data-workspace-reclaim]");
      const control = before.controls[0];
      expect(control.left).toBeGreaterThanOrEqual(before.name.right);
      expect(before.dot.right).toBeLessThan(before.name.left);
      expect(before.dot.top + 4).toBeCloseTo(before.name.top + before.name.height / 2, 1);
      await reclaim.click();
      await page.waitForFunction(() => document.querySelector("[data-workspace-reclaim]").textContent === "Reclaiming…");
      const pending = await rowLayout(row);
      expect(pending.name).toEqual(before.name);
      expect(pending.dot.left).toBe(before.dot.left);
      expect(pending.controls[0].left).toBeGreaterThanOrEqual(pending.name.right);
      expect(pending.dot.right).toBeLessThan(pending.name.left);
      await captureLayout(page, `inbox-status-dot-${label}-reclaiming.png`);
      await page.evaluate(() => { window.__layoutModules.app.App.viewDispose(); window.__layoutModules.feed.stopFeed(); });
    }, { width, height });
  }, 60_000);
}

for (const { label, width, floating, nameX, groupedNameX } of [
  { label: "pinned desktop", width: 1280, floating: false, nameX: 33, groupedNameX: 51 },
  { label: "floating desktop", width: 1280, floating: true, nameX: 46, groupedNameX: 64 },
  { label: "touch mobile", width: 390, floating: true, nameX: 46, groupedNameX: 64 },
]) {
  it(`preserves main's dotted and dotless name columns on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      for (const grouped of [false, true]) {
        await mountStatusDotInbox(page, basePath, { grouped });
        if (floating) await page.evaluate(() => document.body.classList.add("inbox-collapsed", "inbox-popover-open"));
        const dotted = page.locator('[data-key="workspace:layout-device/attention"]');
        const dotless = await rowLayout(page.locator('[data-key="workspace:layout-device/read"]'));
        expect(dotless.dot).toBeNull();
        // Measured in Chromium on main 403fadaba, with the same production fixture.
        expect(dotless.name.left).toBe(grouped ? groupedNameX : nameX);
        for (const active of [false, true]) {
          await dotted.evaluate((row, active) => row.classList.toggle("active", active), active);
          const layout = await rowLayout(dotted);
          expect(layout.name.left).toBe(dotless.name.left);
          expect(layout.dot.right).toBeLessThan(layout.name.left);
          expect(layout.dot.left).toBeGreaterThanOrEqual(layout.row.left + 6);
          expect(layout.dot.top + layout.dot.height / 2).toBeCloseTo(layout.name.top + layout.name.height / 2, 1);
        }
        await mountStatusDotInbox(page, basePath, { grouped: true, folded: true });
        if (floating) await page.evaluate(() => document.body.classList.add("inbox-collapsed", "inbox-popover-open"));
        const head = page.locator(".inbox-project-head");
        const name = await head.locator(".inbox-project-name").boundingBox();
        expect(name.x).toBe(groupedNameX);
        const fold = await head.locator(".inbox-fold").boundingBox();
        const dot = await head.locator(".inbox-status-dot").boundingBox();
        expect(dot.x + dot.width).toBeLessThan(fold.x);
        expect(dot.y + dot.height / 2).toBeCloseTo(name.y + name.height / 2, 1);
      }
    }, { width, height: 800, hasTouch: width === 390 });
  }, 60_000);
}

it("pulses only running dots and respects reduced motion", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountStatusDotInbox(page, basePath);
    const animations = () => page.locator(".inbox-status-dot").evaluateAll((dots) => dots.map((dot) => ({
      running: dot.classList.contains("inbox-status-running"), animation: getComputedStyle(dot).animationName,
      unread: dot.classList.contains("inbox-status-unread"), color: getComputedStyle(dot).backgroundColor,
    })));
    const normal = await animations();
    expect(normal.length).toBeGreaterThan(0);
    for (const dot of normal) expect(dot.animation === "none").toBe(!dot.running);
    const green = normal.filter((dot) => dot.unread);
    const grey = normal.filter((dot) => !dot.unread);
    expect(new Set(green.map((dot) => dot.color)).size).toBe(1);
    expect(new Set(grey.map((dot) => dot.color)).size).toBe(1);
    expect(green[0].color).not.toBe(grey[0].color);
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const dot of await animations()) expect(dot.animation).toBe("none");
  });
});

it("keeps live dots at full strength when muted and offline text is dimmed", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountStatusDotInbox(page, basePath);
    for (const className of ["inbox-muted", "inbox-offline"]) {
      const rows = page.locator(".inbox-entry");
      const paint = await rows.evaluateAll((rows, className) => rows.map((row) => {
        row.classList.remove("inbox-muted", "inbox-offline");
        row.classList.add(className);
        for (const animation of row.getAnimations({ subtree: true })) {
          if (animation instanceof CSSTransition) animation.finish();
        }
        const opacity = (node) => {
          let result = 1;
          for (let current = node; current && current !== row; current = current.parentElement) {
            result *= Number(getComputedStyle(current).opacity);
          }
          return result;
        };
        const dot = row.querySelector(".inbox-status-dot");
        return { key: row.dataset.key, name: opacity(row.querySelector(".stitle")), dot: dot && opacity(dot) };
      }), className);
      for (const row of paint) {
        expect(row.name, `${className}: ${row.key}`).toBeCloseTo(.62);
        if (row.dot !== null) expect(row.dot, `${className}: ${row.key}`).toBe(1);
      }
    }
  });
});
