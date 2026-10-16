import { afterAll, beforeAll, expect, it } from "vitest";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { settled } from "./chatMenuSeed.mjs";
import { flowTerminalShim, mountProjectFlow } from "./projectWorkspaceFlowFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim, flowTerminalShim] }); });
afterAll(async () => { await session?.close(); });

const SHA = "d".repeat(40);
const NEXT_SHA = "e".repeat(40);
const CHIP = '.composer-context [data-context-kind="commit"]';

async function mountScrolledCommit(page, basePath) {
  await mountProjectFlow(page, basePath);
  await settled(page);
  if (!await page.locator(".rail-composer textarea").isVisible()) {
    await page.locator('[data-bubble="agent"][data-agent="project-pin"]').click();
  }
  await page.locator(".rail-composer textarea").waitFor();
  await page.evaluate(async ({ sha, next }) => {
    const { app, cache, directory } = window.__layoutModules;
    app.App.viewingContext.setEnabled(true);
    const held = window.__flowDevices["flow-device"];
    const agents = held.roster.agents.map((agent) => agent.id === "project-pin" ? { ...agent, working: true,
      working_time: { seconds: 129 }, surfaces: {
        subagents: [{ id: "child", label: "Diff review", state: "running" }],
        checklist: [{ id: "step", subject: "Review the diff", state: "in_progress" }],
      } } : agent);
    held.roster = { ...held.roster, agents };
    held.record = { ...held.record, agents };
    await cache.writeCached({ deviceId: "flow-device", entityId: "project-owner", kind: "project-agents" }, held.roster);
    await cache.writeCached({ deviceId: "flow-device", entityId: "project-owner", kind: "row" }, { kind: "project", entity_id: "project-owner", project_id: "p", agents });
    await cache.writeCached({ deviceId: "flow-device", entityId: "", kind: "projects" }, [held.record]);
    const patch = `diff --git a/long.js b/long.js\n--- a/long.js\n+++ b/long.js\n@@ -1,800 +1,800 @@\n${Array.from({ length: 800 }, (_, index) => `-old line ${index}\n+new line ${index}\n`).join("")}`;
    for (const [sourceId, hash] of [["code", sha], ["docs", next]]) {
      const commit = { hash, short: hash.slice(0, 7), subject: "Long commit diff", author: "Developer", time: 1791900000 };
      held.committed.set(sourceId, { commit, patch });
      const entityId = directory.directoryCacheId({ project_id: "p", source_id: sourceId, source_path: `/fixture/${sourceId}` });
      await cache.writeCached({ deviceId: "flow-device", entityId, kind: "log" }, { commits: [commit] });
    }
  }, { sha: SHA, next: NEXT_SHA });
  await page.locator(`.crow[data-hash="${SHA}"]`).evaluate((node) => node.click());
  await page.locator('[data-surface="code"] .file .code').first().waitFor();
  await page.locator('[data-surface="code"] .file.capped .code').first().click();
  await page.waitForFunction(() => document.querySelector('[data-surface="code"] .file:not(.capped):not(.collapsed)'));
  await page.locator(CHIP).waitFor();
  await page.locator(".rail-status-lead").waitFor();
  await page.locator(".rail-status-pills > *").first().waitFor();
  await page.locator('[data-surface="code"] .dscroll.dwindow').evaluate((scroller) => {
    scroller.scrollTop = 6000;
    scroller.dispatchEvent(new Event("scroll"));
  });
  await settled(page);
  expect(await page.locator('[data-surface="code"] .dscroll.dwindow').evaluate((scroller) => scroller.scrollTop)).toBeGreaterThan(5000);
}

async function startSampling(page) {
  await page.evaluate((selector) => {
    const chip = document.querySelector(selector);
    const tray = chip.closest(".composer-context");
    const rows = [".rail-status", ".rail-status-lead", ".rail-status-pills", ".rail-strip"].map((selector) => document.querySelector(selector));
    window.__chipSamples = [];
    window.__chipRemoved = 0;
    window.__contextEvents = [];
    window.__stopContextEvents = window.__layoutModules.app.App.viewingContext.subscribe((context) => {
      window.__contextEvents.push(context?.items || []);
    });
    window.__chipObserver = new MutationObserver((records) => {
      for (const record of records) for (const removed of record.removedNodes) {
        if (removed === chip || removed.contains?.(chip)) window.__chipRemoved += 1;
      }
    });
    window.__chipObserver.observe(tray, { childList: true, subtree: true });
    const sample = () => {
      window.__chipSamples.push({ sameNode: document.querySelector(selector) === chip,
        rows: rows.map((row) => row.getBoundingClientRect().top),
        scrollTop: document.querySelector('[data-surface="code"] .dscroll.dwindow').scrollTop });
      window.__chipFrame = requestAnimationFrame(sample);
    };
    sample();
  }, CHIP);
}

async function finishSampling(page, name) {
  const result = await page.evaluate(() => {
    cancelAnimationFrame(window.__chipFrame);
    window.__chipObserver.disconnect();
    window.__stopContextEvents();
    return { samples: window.__chipSamples, removed: window.__chipRemoved, events: window.__contextEvents };
  });
  if (process.env.BUILD_LAYOUT_SCREENSHOTS) {
    await mkdir(process.env.BUILD_LAYOUT_SCREENSHOTS, { recursive: true });
    await writeFile(join(process.env.BUILD_LAYOUT_SCREENSHOTS, `${name}.json`), JSON.stringify(result, null, 2));
  }
  return result;
}

async function frames(page, name) {
  for (let index = 0; index < 6; index += 1) {
    await captureLayout(page, `${name}-${index}.png`);
    await page.waitForTimeout(70);
  }
  await settled(page);
}

const rowTravel = (samples, index) => {
  const positions = samples.map((sample) => sample.rows[index]);
  return Math.max(...positions) - Math.min(...positions);
};

async function pushCaches(page, revision) {
  await page.evaluate(async (revision) => {
    const { cache, directory } = window.__layoutModules;
    const held = window.__flowDevices["flow-device"];
    await cache.writeCached({ deviceId: "flow-device", entityId: "", kind: "projects" }, [{ ...held.record, repaint: revision }]);
    await cache.writeCached({ deviceId: "flow-device", entityId: "project-owner", kind: "project-agents" }, { ...held.roster, repaint: revision });
    const entityId = directory.directoryCacheId({ project_id: "p", source_id: "code", source_path: "/fixture/code" });
    const status = { ...held.statuses.get("code"), status_key: `live-status-${revision}` };
    held.statuses.set("code", status);
    await cache.writeCached({ deviceId: "flow-device", entityId, kind: "status" }, status);
  }, revision);
}

function expectStable(result) {
  expect(result.samples.length).toBeGreaterThan(10);
  expect(result.events).toEqual([]);
  expect(result.removed).toBe(0);
  expect(result.samples.every((sample) => sample.sameNode)).toBe(true);
  expect(result.samples.every((sample) => sample.scrollTop > 5000)).toBe(true);
  for (let index = 0; index < result.samples[0].rows.length; index += 1) expect(rowTravel(result.samples, index)).toBeLessThanOrEqual(0.5);
}

it("keeps the same commit chip through live project, roster and git cache paints in a long scrolled diff (#637)", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountScrolledCommit(page, basePath);
    await startSampling(page);
    for (let revision = 1; revision <= 4; revision += 1) {
      await pushCaches(page, revision);
      await frames(page, `637-cache-${revision}`);
    }
    const result = await finishSampling(page, "637-cache-samples");
    expectStable(result);

    const selected = await page.evaluate(() => {
      const scroller = document.querySelector('[data-surface="code"] .dscroll.dwindow');
      const bounds = scroller.getBoundingClientRect();
      const code = [...scroller.querySelectorAll(".code")].find((node) => {
        const rect = node.getBoundingClientRect();
        return rect.top > bounds.top && rect.bottom < bounds.bottom;
      });
      const range = document.createRange();
      range.selectNodeContents(code);
      document.getSelection().removeAllRanges();
      document.getSelection().addRange(range);
      document.dispatchEvent(new Event("selectionchange"));
      return { text: document.getSelection().toString(), context: window.__layoutModules.app.App.viewingContext.snapshot() };
    });
    expect(selected.context.items.some((item) => item.kind === "selection" && item.text === selected.text)).toBe(true);
    await settled(page);
    await startSampling(page);
    await pushCaches(page, "selected");
    await frames(page, "637-selected");
    expectStable(await finishSampling(page, "637-selected-samples"));
    expect(await page.evaluate(() => ({ text: document.getSelection().toString(), context: window.__layoutModules.app.App.viewingContext.snapshot() }))).toEqual(selected);
  }, { width: 1280, height: 900 });
}, 60_000);

for (const reducedMotion of ["no-preference", "reduce"]) {
  it(`keeps dismissed context dismissed under cache paints and changes it on navigation (${reducedMotion}, #637)`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion });
      await mountScrolledCommit(page, basePath);
      await page.locator(`${CHIP} button`).click();
      await frames(page, `637-${reducedMotion}-dismiss`);
      await page.evaluate(async () => {
        const { cache } = window.__layoutModules;
        const held = window.__flowDevices["flow-device"];
        await cache.writeCached({ deviceId: "flow-device", entityId: "", kind: "projects" }, [{ ...held.record, repaint: "dismissed" }]);
      });
      await settled(page);
      expect(await page.locator(CHIP).count()).toBe(0);
      await page.locator('[data-directory="assets"]').click();
      await page.locator('[data-directory="code"]').click();
      await frames(page, `637-${reducedMotion}-return`);
      await page.locator(CHIP).waitFor();
      await page.locator('[data-directory="docs"]').click();
      await page.locator(`.crow[data-hash="${NEXT_SHA}"]`).evaluate((node) => node.click());
      await page.waitForFunction((sha) => document.querySelector('.composer-context [data-context-kind="commit"]')?.title === `Commit ${sha.slice(0, 12)}`, NEXT_SHA);
      await frames(page, `637-${reducedMotion}-change`);
    }, { width: 1280, height: 900 });
  }, 60_000);
}
