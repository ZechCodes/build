import assert from "node:assert/strict";
import { it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountModelBadges } from "./modelBadgeSeed.mjs";

it("paints cached model families after comment names and in the rail, preserving their tooltip", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountModelBadges(page, basePath);
    const families = ["Astra", "Fable", "Opus", "Grok"];
    const titles = ["6 Astra · high", "Fable 5.1", "Opus 5.5", "Grok 4.7"];
    for (const [index, family] of families.entries()) {
      const badge = page.locator(`#comment-badge-comment-${index} .agent-model-badge`);
      assert.equal(await badge.count(), 1);
      assert.equal(await badge.textContent(), family);
      assert.equal(await badge.getAttribute("title"), titles[index]);
      assert.equal(await page.locator(`.rail-overview-row .agent-model-badge`).nth(index).textContent(), family);
    }
    assert.equal(await page.locator("#comment-badge-comment-4 .agent-model-badge").count(), 0);
    const geometry = await page.locator("#comment-badge-comment-0 .task-actor-identity").evaluate((identity) => {
      const name = identity.querySelector(".task-actor-name");
      const badge = identity.querySelector(".agent-model-badge");
      return { name: name.getBoundingClientRect().toJSON(), badge: badge.getBoundingClientRect().toJSON(),
        ellipsizes: name.scrollWidth > name.clientWidth };
    });
    assert.ok(Math.abs(geometry.name.y - geometry.badge.y) < 5);
    assert.ok(geometry.ellipsizes);
  }, { width: 390, height: 844, hasTouch: true });
}, 30_000);
