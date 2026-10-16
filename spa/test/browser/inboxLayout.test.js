import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { LAYOUT_DEVICE, LAYOUT_ROWS, disposeInboxLayout, mountInboxLayout, showInboxLayoutFace } from "./inboxLayoutSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

const rowFor = (page, key) => page.locator(`#inbox-list .inbox-entry[data-key="${key}"]`);
const blockFor = (page) => page.locator(`.inbox-project[data-project="${LAYOUT_DEVICE}/live-project"]`);

async function measureRow(row) {
  await row.scrollIntoViewIfNeeded();
  return row.evaluate((element) => {
    const box = (node) => {
      if (!node) return null;
      const { left, right, top, bottom, width } = node.getBoundingClientRect();
      return { left, right, top, bottom, width };
    };
    const project = element.querySelector(".inbox-project-line");
    const name = element.querySelector(".inbox-name");
    const facts = element.querySelector(".inbox-facts");
    return { project: box(project), name: box(name), facts: box(facts), row: box(element),
      title: element.querySelector(".stitle")?.textContent, projectText: project?.textContent.trim(),
      factsText: facts?.textContent.trim(), projectInName: Boolean(name?.querySelector(".inbox-tag")),
      recent: Boolean(element.closest(".inbox-recent")) };
  });
}

for (const width of [1280, 390]) {
  it(`puts project, name and activity on separate lines for every inbox kind, including Recent, at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountInboxLayout(page, basePath);
      await showInboxLayoutFace(page, "inbox");
      for (const entry of LAYOUT_ROWS) {
        const row = rowFor(page, entry.key);
        const measured = await measureRow(row);
        expect(measured.projectText, entry.key).toBe(entry.project);
        expect(measured.title, entry.key).toBe(entry.name);
        expect(measured.projectInName, entry.key).toBe(false);
        expect(measured.recent, entry.key).toBe(entry.recent);
        expect(measured.project.width, entry.key).toBeGreaterThan(0);
        expect(measured.name.width, entry.key).toBeGreaterThan(0);
        expect(measured.facts.width, entry.key).toBeGreaterThan(0);
        expect(measured.name.top, entry.key).toBeGreaterThanOrEqual(measured.project.bottom);
        expect(measured.facts.top, entry.key).toBeGreaterThanOrEqual(measured.name.bottom);
        expect(measured.project.left, entry.key).toBeGreaterThanOrEqual(measured.row.left);
        expect(measured.project.right, entry.key).toBeLessThanOrEqual(measured.row.right);
        expect(measured.factsText, entry.key).not.toBe("");
        if (entry.key.startsWith("workspace:")) {
          expect(await row.locator("[data-workspace-done]").count(), entry.key).toBe(entry.recent ? 0 : 1);
        }
      }
      await disposeInboxLayout(page);
    }, { width, height: 960, hasTouch: width === 390 });
  }, 30_000);

  it(`includes an ordinary project row in its block and keeps headings keyboard disclosures at ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountInboxLayout(page, basePath);
      await showInboxLayoutFace(page, "projects");
      for (const entry of LAYOUT_ROWS) {
        const row = rowFor(page, entry.key);
        expect(await row.count(), entry.key).toBe(1);
        expect(await row.locator(".inbox-project-line, .inbox-tag").count(), entry.key).toBe(0);
        expect(await row.locator(".stitle").textContent(), entry.key).toBe(entry.name);
        expect(await row.evaluate((element) => Boolean(element.closest(".inbox-recent"))), entry.key).toBe(entry.recent);
      }
      const own = rowFor(page, `project-agent:${LAYOUT_DEVICE}/live-project`);
      const measured = await measureRow(own);
      expect(measured.factsText).toBe("Getting started");
      expect(measured.facts.top).toBeGreaterThanOrEqual(measured.name.bottom);
      expect(await own.locator("button").count()).toBe(0);

      const heading = blockFor(page).locator(".inbox-project-name");
      expect(await heading.getAttribute("data-project-fold")).toBe(`${LAYOUT_DEVICE}/live-project`);
      expect(await heading.getAttribute("data-project-open")).toBeNull();
      await page.evaluate(() => {
        window.__inboxLayoutRoute = JSON.stringify(window.__layoutModules.app.App.route);
        window.__inboxLayoutSeen = window.__inboxLayoutCalls.filter((call) => call.method === "entity.seen").length;
      });
      await heading.focus();
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => document.querySelector('.inbox-project[data-project$="/live-project"]')
        ?.classList.contains("inbox-folded"));
      expect(await heading.getAttribute("aria-expanded")).toBe("false");
      expect(await own.isVisible()).toBe(false);
      await page.keyboard.press("Space");
      await page.waitForFunction(() => !document.querySelector('.inbox-project[data-project$="/live-project"]')
        ?.classList.contains("inbox-folded"));
      expect(await heading.getAttribute("aria-expanded")).toBe("true");
      expect(await own.isVisible()).toBe(true);
      expect(await page.evaluate(() => JSON.stringify(window.__layoutModules.app.App.route) === window.__inboxLayoutRoute)).toBe(true);
      expect(await page.evaluate(() => window.__inboxLayoutCalls.filter((call) => call.method === "entity.seen").length))
        .toBe(await page.evaluate(() => window.__inboxLayoutSeen));
      expect(await page.evaluate(() => document.activeElement?.classList.contains("inbox-project-name"))).toBe(true);
      await disposeInboxLayout(page);
    }, { width, height: 960, hasTouch: width === 390 });
  }, 30_000);
}

it.each(["inbox", "projects"])("preserves workspace, task and project row nodes across a live cached reorder in %s", async (face) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountInboxLayout(page, basePath);
    await showInboxLayoutFace(page, face);
    const key = `workspace:${LAYOUT_DEVICE}/live-workspace`;
    await page.evaluate(async ({ key, deviceId }) => {
      window.__inboxLayoutNodes = [...document.querySelectorAll("#inbox-list .inbox-entry")]
        .map((row) => [row.dataset.key, row]);
      const address = { deviceId, entityId: "", kind: "workspaces" };
      const { value: rows } = await window.__layoutModules.cache.readCached(address);
      await window.__layoutModules.cache.writeCached(address, rows.map((row) =>
        `workspace:${row.workspaceKey}` === key ? { ...row, session_started_ms: Date.now(), last_activity_ms: Date.now() } : row));
    }, { key, deviceId: LAYOUT_DEVICE });
    const container = face === "inbox" ? "#inbox-list" : `.inbox-project[data-project="${LAYOUT_DEVICE}/live-project"] .inbox-project-rows`;
    await page.waitForFunction(({ container, key }) => [...document.querySelector(container).children]
      .filter((row) => row.matches(".inbox-entry")).at(-1)?.dataset.key === key, { container, key });
    expect(await page.evaluate(() => window.__inboxLayoutNodes.every(([key, node]) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key === key) === node))).toBe(true);
    await disposeInboxLayout(page);
  }, { width: 1280, height: 960 });
}, 30_000);
