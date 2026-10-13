import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountProjectTerminal, terminalShim } from "./projectWorkspaceTerminalFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim, terminalShim] }); });
afterAll(async () => { await session?.close(); });

const railToggle = (page) => page.getByRole("button", { name: "Terminal", exact: true });
const callsTo = (page, method) => page.evaluate((method) => window.__terminalCalls.filter((call) => call.method === method), method);

it("keeps the cached console unchanged across terminal connection transitions", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectTerminal(page, basePath);
    await railToggle(page).click();
    await page.locator(".project-terminal-screen").waitFor();
    const before = await page.locator("#console-region").innerHTML();
    await page.evaluate(() => { window.__keptTerminal = document.querySelector(".console-pane"); });
    for (const status of ["disconnected", "connecting", "connected"]) {
      await page.evaluate(async (status) => {
        window.__setTerminalStatus(status);
        // Reconnection can schedule a cache read; let that finish as well.
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      }, status);
      expect(await page.locator("#console-region").innerHTML()).toBe(before);
      expect(await page.evaluate(() => window.__keptTerminal === document.querySelector(".console-pane"))).toBe(true);
    }
    expect(await callsTo(page, "term.attach")).toHaveLength(1);
  });
}, 60_000);

for (const rosterChanges of [false, true]) {
  it(`keeps New terminal keyboard focus through ${rosterChanges ? "roster insertions requiring a move" : "an unrelated cache write"}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountProjectTerminal(page, basePath);
      const add = page.getByRole("button", { name: "New terminal", exact: true });
      await add.focus();
      await page.evaluate(async (rosterChanges) => {
        const { cache, directory } = window.__layoutModules;
        const region = document.querySelector("#console-region");
        const strip = region.querySelector(".console-tabs");
        window.__keptNewTerminal = strip.querySelector(".console-new");
        window.__consoleListMoves = [];
        const moves = new MutationObserver((records) => window.__consoleListMoves.push(...records.map((record) => record.type)));
        moves.observe(strip, { childList: true });
        // Exercise the fallback for browsers without state-preserving moves.
        if (rosterChanges) strip.moveBefore = undefined;
        const painted = new Promise((resolve) => {
          const observer = new MutationObserver(() => { observer.disconnect(); resolve(); });
          observer.observe(region, { attributes: true, attributeFilter: ["data-size"] });
        });
        if (rosterChanges) {
          const scope = { project_id: "p", source_id: "notes", source_path: "/project/notes" };
          await cache.writeCached({ deviceId: "console-device", entityId: directory.directoryCacheId(scope), kind: "terminals" },
            { tabs: [{ term_id: "term-notes" }, { term_id: "term-external", name: "External" }] });
        } else {
          await cache.writeCached({ deviceId: "console-device", entityId: "unrelated", kind: "tasks" }, []);
        }
        await painted;
        moves.disconnect();
      }, rosterChanges);
      expect(await add.evaluate((button) => button === document.activeElement)).toBe(true);
      expect(await page.evaluate(() => window.__keptNewTerminal === document.querySelector(".console-new"))).toBe(true);
      expect(await page.locator(".console-tabs > :last-child").getAttribute("aria-label")).toBe("New terminal");
      if (rosterChanges) expect(await page.locator(".console-tab-name").allTextContents()).toContain("Notes · External");
      else expect(await page.evaluate(() => window.__consoleListMoves)).toEqual([]);
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__terminalCalls.some((call) => call.method === "term.create"));
      expect(await callsTo(page, "term.create")).toHaveLength(1);
    });
  }, 60_000);
}

for (const width of [390, 1280]) {
  for (const theme of ["light", "dark"]) {
    it(`keeps source shells in the existing console at ${width}px in ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await mountProjectTerminal(page, basePath, { theme });
        const toggle = railToggle(page);
        await toggle.waitFor({ timeout: 3000 });
        expect(await toggle.getAttribute("aria-expanded")).toBe("false");
        expect(await toggle.evaluate((button) => button.closest('[role="tablist"]'))).toBeNull();
        expect(await page.locator('[role="tab"][data-tab="terminal"]').count()).toBe(0);
        expect(await callsTo(page, "term.create")).toEqual([]);
        expect(await callsTo(page, "term.attach")).toEqual([]);
        await toggle.click();
        await page.waitForFunction(() => document.querySelector("#console-region")?.dataset.size === "half");
        expect(await toggle.getAttribute("aria-expanded")).toBe("true");
        expect(await page.locator(".console-tab-name").allTextContents()).toEqual(["Code · Terminal 1", "Notes · Terminal 1"]);
        await page.getByLabel("Terminal source", { exact: true }).selectOption("notes");
        expect(await page.locator(".console-tab.active .console-tab-name").textContent()).toBe("Code · Terminal 1");
        await page.getByRole("button", { name: "New terminal", exact: true }).click();
        await page.waitForFunction(() => window.__terminalCalls.some((call) => call.method === "term.create"));
        expect(await callsTo(page, "term.create")).toEqual([{ method: "term.create", params: { project_id: "p", source_id: "notes", source_path: "/project/notes" } }]);
        await page.getByRole("button", { name: "Code · Terminal 1", exact: true }).click();
        await page.locator(".project-terminal-screen").waitFor();
        await page.evaluate(() => { window.__keptTerminal = document.querySelector(".console-pane"); });
        await page.locator('[data-tab="files"]').click();
        await page.waitForFunction(() => window.__layoutModules.app.App.route.tab === "files");
        expect(await page.locator("#console-region").getAttribute("data-size")).toBe("half");
        expect(await page.getByLabel("Terminal source", { exact: true }).inputValue()).toBe("notes");
        expect(await page.locator(".console-tab.active .console-tab-name").textContent()).toBe("Code · Terminal 1");
        expect(await page.evaluate(() => window.__layoutModules.app.App.route.name)).toBe("project");
        expect(await page.evaluate(() => window.__keptTerminal === document.querySelector(".console-pane"))).toBe(true);
        const layout = await page.evaluate(() => ({ width: innerWidth, scroll: document.documentElement.scrollWidth,
          rail: document.querySelector('[data-rail-terminal]').getBoundingClientRect().width,
          console: document.querySelector("#console-region").getBoundingClientRect().width }));
        expect(layout.scroll).toBeLessThanOrEqual(layout.width + 1);
        expect(layout.rail).toBeGreaterThan(24);
        expect(layout.console).toBeGreaterThan(200);
        await captureLayout(page, `project-terminal-${width}-${theme}.png`);
      }, { width, height: 840 });
    }, 60_000);
  }
}

it("toggles Terminal by keyboard with an accessible state and keeps its face route", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectTerminal(page, basePath);
    const toggle = railToggle(page);
    await toggle.waitFor({ timeout: 3000 });
    await toggle.focus();
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => document.querySelector('[data-rail-terminal]')?.getAttribute("aria-expanded") === "true");
    await page.keyboard.press("Space");
    await page.waitForFunction(() => document.querySelector('[data-rail-terminal]')?.getAttribute("aria-expanded") === "false");
    expect(await toggle.evaluate((button) => button === document.activeElement)).toBe(true);
    expect(await page.evaluate(() => window.__layoutModules.app.App.route.tab)).toBe("workspaces");
    expect(await callsTo(page, "term.create")).toEqual([]);
  }, { width: 390, height: 840 });
}, 60_000);

it("paints cached source shells on an older bridge without dispatching new terminal selectors", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectTerminal(page, basePath, { supported: false });
    const toggle = railToggle(page);
    await toggle.waitFor({ timeout: 3000 });
    expect(await page.locator(".console-tab-name").allTextContents()).toEqual(["Code · Terminal 1", "Notes · Terminal 1"]);
    await toggle.click();
    await page.waitForFunction(() => document.querySelector("#console-region")?.dataset.size === "half");
    expect(await callsTo(page, "term.list")).toEqual([]);
    expect(await callsTo(page, "term.create")).toEqual([]);
    expect(await callsTo(page, "term.attach")).toEqual([]);
  }, { width: 390, height: 840 });
}, 60_000);
