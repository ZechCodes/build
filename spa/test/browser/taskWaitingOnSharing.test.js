import { afterAll, beforeAll, expect, it } from "vitest";
import { loadBrowserModules, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountWaitingOnFixture } from "./taskWaitingOnHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

it("shares refused-history evidence over the existing channel and refuses the other tab's real picker", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath, { old: true, legacy: true });
    const opened = page.waitForEvent("popup");
    await page.evaluate(() => window.open(location.href, "history-peer"));
    const other = await opened;
    try {
      await other.goto(page.url());
      await mountWaitingOnFixture(other, basePath, { old: true, legacy: true });
      await loadBrowserModules(other, { history: "src/core/taskWaitingOnSupport.js" }, basePath);
      await page.evaluate(async () => {
        const put = IDBObjectStore.prototype.put;
        window.__waiting.historyRefused = false;
        IDBObjectStore.prototype.put = function (value, key) {
          if (String(key).includes("task-waiting-on-support") && value.value.everWaitingOn) {
            window.__waiting.historyRefused = true;
            throw new DOMException("Injected history refusal", "QuotaExceededError");
          }
          return put.call(this, value, key);
        };
        window.__waiting.restorePut = () => { IDBObjectStore.prototype.put = put; };
        await window.__waiting.greet(false);
      });
      await page.waitForFunction(() => window.__waiting.historyRefused);
      await other.waitForFunction(async () => await window.__layoutModules.history.readTaskWaitingOnEverSupported("device-waiting") === true);
      expect(await other.evaluate(() => window.__layoutModules.history.readTaskWaitingOnSupport("device-waiting"))).toBe(false);
      await page.evaluate(() => window.__waiting.restorePut());
      await other.evaluate(() => window.__waiting.greet(true));
      await other.locator("[data-task-assign]").click();
      await other.locator("[data-assignee-select]").selectOption("new_workspace");
      await other.locator("#task-assign-note").fill("Keep the shared capability evidence");
      await other.locator("[data-assign-go]").click();
      await other.waitForFunction(() => document.querySelector(".create-error")?.textContent.includes("Retry or refresh"));
      expect(await other.evaluate(() => window.__waiting.calls.filter(({ method }) => method === "tasks.assign"))).toEqual([]);
      expect(await other.locator("#task-assign-note").inputValue()).toBe("Keep the shared capability evidence");
    } finally { await other.close(); }
  });
}, 30_000);
