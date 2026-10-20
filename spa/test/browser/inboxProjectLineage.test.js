import assert from "node:assert/strict";
import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { mountProjectLineageInbox, stopProjectLineageChild } from "./inboxProjectLineageSeed.mjs";
import { spawnedProjectAliasFixture } from "../inboxProjectLineageFixture.js";

let session;
beforeAll(async () => { session = await openLayoutSession(); }, 30_000);
afterAll(async () => { await session?.close(); });

it("the cached inbox row agrees with the chat's three running agents and repaints child stops", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectLineageInbox(page, basePath);
    // Capture before asserting so a failing regression preserves the actual row.
    await captureLayout(page, "inbox-project-lineage.png");
    const row = page.locator(".inbox-project-agent");
    assert.equal(await page.locator("#agent-rail .surface-pill-count").textContent(), "3");
    assert.equal(await row.locator(".stitle").textContent(), "Bridge roll and #564");
    assert.equal(await row.locator(".inbox-facts").textContent(), "3 running");
    assert.equal(await row.locator(".inbox-status-running").count(), 1);
    await page.evaluate(async () => {
      const { cache } = window.__layoutModules;
      const { project, roster, pin } = window.__projectLineageFixture;
      await cache.writeCached({ deviceId: project.deviceId, entityId: project.entity_id, kind: "project-agents" }, {
        ...roster, agents: [{ ...pin, topic: "Review the inbox fix" }],
      });
    });
    await page.waitForFunction(() => document.querySelector(".inbox-project-agent .stitle")?.textContent === "Review the inbox fix");
    await stopProjectLineageChild(page, 0);
    await page.waitForFunction(() => document.querySelector(".inbox-project-agent .inbox-facts")?.textContent === "2 running");
    await stopProjectLineageChild(page, 1);
    await stopProjectLineageChild(page, 2);
    await page.waitForFunction(() => document.querySelector(".inbox-project-agent .inbox-facts")?.textContent === "0 running");
    assert.equal(await row.locator(".inbox-status-running").count(), 0);
    await captureLayout(page, "inbox-project-lineage-idle.png");
  }, { width: 1320, height: 720 });
}, 30_000);

for (const repeated of [false, true]) it(`lists a spawned canonical worker once in the browser Agents tab (exact repeat: ${repeated})`, async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectLineageInbox(page, basePath, { fixture: spawnedProjectAliasFixture(repeated) });
    expect.soft(await page.locator(".inbox-project-agent .inbox-facts").textContent()).toBe("1 running");
    expect.soft(await page.locator("#agent-rail .surface-pill-count").textContent()).toBe("1");
    await page.locator("#agent-rail .surface-pill").click();
    await page.locator(".surface-build-agents .surface-row").first().waitFor();
    expect(await page.locator(".surface-build-agents .surface-row").count()).toBe(1);
    await captureLayout(page, `inbox-project-lineage-alias-${repeated}.png`);
  }, { width: 1320, height: 720 });
}, 30_000);
