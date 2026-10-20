import assert from "node:assert/strict";
import { afterAll, beforeAll, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { mountProjectLineageInbox, stopProjectLineageChild } from "./inboxProjectLineageSeed.mjs";

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
    await page.waitForFunction(() => document.querySelector(".inbox-project-agent .inbox-facts")?.textContent === "Getting started · 0 running");
    assert.equal(await row.locator(".inbox-status-running").count(), 0);
    await captureLayout(page, "inbox-project-lineage-idle.png");
  }, { width: 1320, height: 720 });
}, 30_000);
