import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

const shellMarkup = readFileSync(resolve("index.html"), "utf8")
  .match(/<body>([\s\S]*)<\/body>/)[1];

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

const layouts = [
  { width: 390, directory: false },
  { width: 1280, directory: true },
  { width: 390, directory: true },
  { width: 1280, directory: false },
];

async function mountCollapsedShell(page, basePath, { directory, inset }) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { left: inset } });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mountLayout(page, shellMarkup, { basePath });
  if (directory) {
    await loadBrowserModules(page, { directory: "src/core/directoryRail.js" }, basePath);
    await page.evaluate(() => {
      const { paintDirectoryRail, PROJECT_TABS } = window.__layoutModules.directory;
      paintDirectoryRail(document.querySelector("#dir-rail"), {
        tabs: PROJECT_TABS, active: "tasks", onSelect: () => {}, sidebar: false,
      });
    });
  }
  await page.evaluate(() => document.body.classList.add("inbox-collapsed"));
}

async function showCaret(page, state) {
  await page.evaluate((state) => {
    document.body.classList.remove("inbox-popover-open", "inbox-peek");
    document.body.classList.add(state);
    return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  }, state);
  await expect(page.locator("#inbox-rail").isVisible()).resolves.toBe(true);
}

const geometry = (page) => page.evaluate(() => {
  const rail = document.querySelector("#inbox-rail");
  const rect = rail.getBoundingClientRect();
  const caret = getComputedStyle(rail, "::before");
  const number = (property) => parseFloat(caret[property]);
  // Rotation around the box's centre preserves centre-x. Include borders:
  // the universal box-sizing rule applies to elements, not pseudo-elements.
  const caretWidth = number("width") + (caret.boxSizing === "border-box" ? 0 :
    number("paddingLeft") + number("paddingRight") + number("borderLeftWidth") + number("borderRightWidth"));
  const transform = new DOMMatrixReadOnly(caret.transform);
  const toggle = document.querySelector("#inbox-open").getBoundingClientRect();
  return {
    caret: rect.left + rail.clientLeft + number("left") + caretWidth / 2 + transform.m41,
    toggle: toggle.left + toggle.width / 2,
    caretWidth, content: caret.content,
    popover: { left: rect.left, top: rect.top, width: rect.width, height: rect.height },
  };
});

async function expectCaretAligned(page, label) {
  const measured = await geometry(page);
  console.log(`Inbox caret centre-x ${label}: ${JSON.stringify(measured)}`);
  expect(measured.content).toBe('""');
  expect(measured.caretWidth).toBeGreaterThan(0);
  expect.soft(Math.abs(measured.caret - measured.toggle), label).toBeLessThanOrEqual(1);
  return measured;
}

for (const layout of layouts) {
  for (const inset of [0, 20]) {
    const label = `${layout.width}px ${layout.directory ? "with" : "without"} directory rail, safe-area ${inset}px`;
    it(`centres the popover and peek carets at ${label}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await mountCollapsedShell(page, basePath, { ...layout, inset });
        await showCaret(page, "inbox-popover-open");
        const popover = await expectCaretAligned(page, `${label}, popover`);
        await captureLayout(page, `inbox-caret-${layout.width}-${layout.directory ? "rail" : "no-rail"}-${inset}.png`);
        await showCaret(page, "inbox-peek");
        const peek = await expectCaretAligned(page, `${label}, peek`);
        expect(peek.popover).toEqual(popover.popover);
      }, { width: layout.width, height: 820 });
    });
  }
}

it("follows shared geometry changes and removal of the standing directory rail", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountCollapsedShell(page, basePath, { directory: true, inset: 20 });
    await page.evaluate(() => {
      document.querySelector("#shell").style.setProperty("--region-gap", "14px");
      document.documentElement.style.setProperty("--dir-rail", "48px");
      document.documentElement.style.setProperty("--inbox-toggle", "32px");
    });
    await showCaret(page, "inbox-popover-open");
    const withRail = await expectCaretAligned(page, "changed shared sizes, with rail");
    await page.evaluate(() => document.querySelector("#dir-rail").replaceChildren());
    await showCaret(page, "inbox-peek");
    const withoutRail = await expectCaretAligned(page, "changed shared sizes, without rail");
    expect(withoutRail.popover).toEqual(withRail.popover);
    expect(withoutRail.toggle - withRail.toggle).toBe(20);
  }, { width: 1280, height: 820 });
});
