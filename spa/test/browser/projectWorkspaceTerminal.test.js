import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountProjectTerminal, terminalShim } from "./projectWorkspaceTerminalFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim, terminalShim] }); });
afterAll(async () => { await session?.close(); });

const consoleToggle = (page) => page.locator("#console-toggle");
const callsTo = (page, method) => page.evaluate((method) => window.__terminalCalls.filter((call) => call.method === method), method);

for (const width of [390, 1280]) {
  it(`shows a sole source without shifting the console when cached sources change at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountProjectTerminal(page, basePath, { sourceCount: 1 });
      const picker = page.locator(".console-source");
      const icon = page.locator(".console-source-icon svg");
      const add = page.getByRole("button", { name: "New terminal", exact: true });
      expect(await icon.count()).toBe(1);
      expect(await picker.isVisible()).toBe(true);
      expect(await icon.isVisible()).toBe(true);
      expect(await picker.inputValue()).toBe("code");
      await add.focus();
      await page.keyboard.press("Tab");
      expect(await picker.evaluate((select) => select === document.activeElement)).toBe(true);
      await consoleToggle(page).click();
      await page.locator(".project-terminal-screen").waitFor();
      await page.waitForFunction(() => document.querySelector("#console-region").getAnimations().length === 0);
      await page.evaluate(() => {
        window.__keptTerminal = document.querySelector(".console-pane");
        window.__keptPicker = document.querySelector(".console-source");
      });
      const geometry = () => page.evaluate(() => [".console-tabs", ".console-new", ".console-controls", ".console-grow", "#console-region"].map((selector) => {
        const { x, y, width, height } = document.querySelector(selector).getBoundingClientRect();
        return { x, y, width, height };
      }));
      const before = await geometry();
      for (const sourceCount of [2, 1]) {
        await page.evaluate(async (sourceCount) => {
          const { cache, directory } = window.__layoutModules;
          const address = { deviceId: "console-device", entityId: "", kind: "projects" };
          const project = (await cache.readCached(address)).value[0];
          const notes = { id: "notes", name: "Notes", path: "/project/notes", is_git: false };
          const scope = { project_id: "p", source_id: notes.id, source_path: notes.path };
          await cache.writeCached({ ...address, entityId: directory.directoryCacheId(scope), kind: "terminals" }, { tabs: [] });
          await cache.writeCached(address, [{ ...project, sources: [project.sources[0], notes].slice(0, sourceCount) }]);
        }, sourceCount);
        await page.waitForFunction((count) => document.querySelector(".console-source")?.options.length === count, sourceCount);
        expect(await picker.isVisible()).toBe(true);
        expect(await icon.isVisible()).toBe(true);
        await picker.focus();
        expect(await picker.evaluate((select) => select === document.activeElement)).toBe(true);
        expect(await geometry()).toEqual(before);
        expect(await page.evaluate(() => window.__keptTerminal === document.querySelector(".console-pane"))).toBe(true);
        expect(await page.evaluate(() => window.__keptPicker === document.querySelector(".console-source"))).toBe(true);
        if (sourceCount === 2) {
          await picker.selectOption("notes");
          expect(await page.locator(".console-tab.active .console-tab-name").textContent()).toBe("Code · Terminal 1");
        }
      }
      expect(await picker.inputValue()).toBe("code");
      expect(await callsTo(page, "term.attach")).toHaveLength(1);
      await captureLayout(page, `project-terminal-single-source-${width}.png`);
    }, { width, height: 840 });
  }, 60_000);
}

it("keeps the cached console unchanged across terminal connection transitions", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectTerminal(page, basePath);
    await consoleToggle(page).click();
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
        if (rosterChanges) {
          const scope = { project_id: "p", source_id: "notes", source_path: "/project/notes" };
          await cache.writeCached({ deviceId: "console-device", entityId: directory.directoryCacheId(scope), kind: "terminals" },
            { tabs: [{ term_id: "term-notes" }, { term_id: "term-external", name: "External" }] });
        } else {
          await cache.writeCached({ deviceId: "console-device", entityId: "unrelated", kind: "tasks" }, []);
        }
        // writeCached resolves after commit and synchronous cache delivery.
        // An unrelated write intentionally causes no console repaint.
        window.__stopConsoleMoves = () => moves.disconnect();
      }, rosterChanges);
      if (rosterChanges) await page.waitForFunction(() => [...document.querySelectorAll(".console-tab-name")].some((tab) => tab.textContent === "Notes · External"));
      await page.evaluate(() => window.__stopConsoleMoves());
      expect(await add.evaluate((button) => button === document.activeElement)).toBe(true);
      expect(await page.evaluate(() => window.__keptNewTerminal === document.querySelector(".console-new"))).toBe(true);
      expect(await page.locator(".console-tabs > :last-child").getAttribute("aria-label")).toBe("New terminal");
      if (rosterChanges) expect(await page.locator(".console-tab-name").allTextContents()).toContain("Notes · External");
      else expect(await page.evaluate(() => window.__consoleListMoves)).toEqual([]);
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => document.querySelector("#console-region")?.dataset.size === "half"
        && document.querySelector(".console-tab.active")?.dataset.key.includes("term-created"));
      await page.locator(".project-terminal-screen").waitFor();
      expect(await callsTo(page, "term.create")).toHaveLength(1);
    });
  }, 60_000);
}

for (const width of [390, 1280]) {
  for (const theme of ["light", "dark"]) {
    it(`keeps source shells in the existing console at ${width}px in ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await mountProjectTerminal(page, basePath, { theme });
        const toggle = consoleToggle(page);
        await toggle.waitFor({ timeout: 3000 });
        expect(await toggle.getAttribute("aria-expanded")).toBe("false");
        expect(await toggle.evaluate((button) => button.closest('[role="tablist"]'))).toBeNull();
        expect(await page.locator('[data-rail-terminal]').count()).toBe(0);
        const folder = await page.evaluate(() => {
          const icon = document.querySelector(".console-source-icon svg");
          if (!icon) return null;
          const picker = document.querySelector(".console-source");
          const glyph = icon.getBoundingClientRect();
          const field = picker.getBoundingClientRect();
          const style = getComputedStyle(picker);
          return { glyph: icon.innerHTML, filesGlyph: document.querySelector('[data-tab="files"] svg').innerHTML,
            width: glyph.width, height: glyph.height, fontSize: parseFloat(style.fontSize),
            center: glyph.y + glyph.height / 2, fieldCenter: field.y + field.height / 2,
            left: glyph.x, fieldLeft: field.x, right: glyph.right, textLeft: field.x + parseFloat(style.paddingLeft),
            decorative: icon.closest('[aria-hidden="true"]') !== null };
        });
        expect(folder).not.toBeNull();
        expect(folder.glyph).toBe(folder.filesGlyph);
        expect(folder.width).toBe(folder.fontSize);
        expect(folder.height).toBe(folder.fontSize);
        expect(folder.center).toBeCloseTo(folder.fieldCenter, 0);
        expect(folder.left).toBeGreaterThan(folder.fieldLeft);
        expect(folder.right).toBeLessThan(folder.textLeft);
        expect(folder.decorative).toBe(true);
        // A click on the decorative glyph still reaches the native select.
        await page.mouse.click(folder.left + folder.width / 2, folder.center);
        await page.waitForFunction(() => document.querySelector(".console-source:open"));
        await page.keyboard.press("Escape");
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
          rail: document.querySelector('#dir-rail').getBoundingClientRect().width,
          console: document.querySelector("#console-region").getBoundingClientRect().width }));
        expect(layout.scroll).toBeLessThanOrEqual(layout.width + 1);
        expect(layout.rail).toBeGreaterThan(24);
        expect(layout.console).toBeGreaterThan(200);
        await captureLayout(page, `project-terminal-${width}-${theme}.png`);
      }, { width, height: 840 });
    }, 60_000);
  }
}

it("toggles the Console tray by keyboard with an accessible state and keeps its face route", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectTerminal(page, basePath);
    const toggle = consoleToggle(page);
    await toggle.waitFor({ timeout: 3000 });
    await toggle.focus();
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => document.querySelector('#console-toggle')?.getAttribute("aria-expanded") === "true");
    await page.keyboard.press("Space");
    await page.waitForFunction(() => document.querySelector('#console-toggle')?.getAttribute("aria-expanded") === "false");
    expect(await toggle.evaluate((button) => button === document.activeElement)).toBe(true);
    expect(await page.evaluate(() => window.__layoutModules.app.App.route.tab)).toBe("workspaces");
    expect(await callsTo(page, "term.create")).toEqual([]);
  }, { width: 390, height: 840 });
}, 60_000);

it("paints cached source shells on an older bridge without dispatching new terminal selectors", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectTerminal(page, basePath, { supported: false });
    const toggle = consoleToggle(page);
    await toggle.waitFor({ timeout: 3000 });
    expect(await page.locator(".console-tab-name").allTextContents()).toEqual(["Code · Terminal 1", "Notes · Terminal 1"]);
    await toggle.click();
    await page.waitForFunction(() => document.querySelector("#console-region")?.dataset.size === "half");
    expect(await callsTo(page, "term.list")).toEqual([]);
    expect(await callsTo(page, "term.create")).toEqual([]);
    expect(await callsTo(page, "term.attach")).toEqual([]);
  }, { width: 390, height: 840 });
}, 60_000);
