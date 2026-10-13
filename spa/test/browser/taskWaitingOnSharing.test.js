import { afterAll, beforeAll, expect, it } from "vitest";
import { loadBrowserModules, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountWaitingOnFixture } from "./taskWaitingOnHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

it("refuses the real picker when channel evidence arrives during its pending history read", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath, { old: true, legacy: true });
    await loadBrowserModules(page, { history: "src/core/taskWaitingOnSupport.js", cache: "src/core/localCache.js" }, basePath);
    expect((await page.evaluate(() => window.__layoutModules.cache.readCached(
      window.__layoutModules.history.taskWaitingOnSupportAddress("device-waiting"),
    ))).value).toEqual({ waitingOn: false, everWaitingOn: false });
    await page.locator("[data-task-assign]").click();
    await page.locator("[data-assignee-select]").selectOption("new_workspace");
    await page.locator("#task-assign-note").fill("Keep evidence received while reading history");
    await page.evaluate(() => {
      const fixture = window.__waiting;
      const get = IDBObjectStore.prototype.get;
      fixture.releaseHistoryRead = false;
      // Keep the actual read transaction open after its older record arrives.
      // Repeated requests use the original get, so only admission is intercepted.
      IDBObjectStore.prototype.get = function (key) {
        const request = get.call(this, key);
        if (this.transaction.mode === "readonly" && String(key).includes("task-waiting-on-support")) {
          IDBObjectStore.prototype.get = get;
          request.addEventListener("success", () => {
            fixture.historyReadPending = true;
            const keepAlive = () => {
              if (!fixture.releaseHistoryRead) get.call(this, key).addEventListener("success", keepAlive);
            };
            keepAlive();
          });
        }
        return request;
      };
    });
    await page.locator("[data-assign-go]").click();
    await page.waitForFunction(() => window.__waiting.historyReadPending);
    await page.evaluate(() => {
      const channel = new BroadcastChannel("build-cache");
      channel.postMessage({ key: "device-waiting||task-waiting-on-support", evidence: { waitingOnSeen: true } });
      channel.close();
    });
    await page.waitForFunction(async () => await window.__layoutModules.history.readTaskWaitingOnEverSupported("device-waiting") === true);
    expect(await page.evaluate(() => window.__waiting.calls.filter(({ method }) => method === "tasks.assign"))).toEqual([]);
    await page.evaluate(() => { window.__waiting.releaseHistoryRead = true; });
    await page.waitForFunction(() => document.querySelector(".create-error")?.textContent
      || window.__waiting.calls.some(({ method }) => method === "tasks.assign"));
    expect(await page.evaluate(() => window.__waiting.calls.filter(({ method }) => method === "tasks.assign"))).toEqual([]);
    expect(await page.locator(".create-error").textContent()).toContain("Retry or refresh");
    expect(await page.locator("#task-assign-note").inputValue()).toBe("Keep evidence received while reading history");
    expect(await page.locator("[data-assignee-select]").inputValue()).toBe("new_workspace");
  });
}, 30_000);

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
