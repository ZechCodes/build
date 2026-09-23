import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const AVAILABLE = {
  running_version: "0.2.0", platform: "linux-x86_64", development_build: false,
  latest_release: { version: "0.3.0" }, last_checked_at: "2026-09-23T12:00:00Z",
  state: "available", last_error: null, update_available: true, can_install: true,
};
const QUEUED = { ...AVAILABLE, state: "scheduled_when_idle" };
const FAILED = { ...AVAILABLE, state: "failed", last_error: "Health check failed; previous bridge restored." };

async function withTwoTabs(check, { actionStatus = null } = {}) {
  await withLayoutPage(async ({ page, basePath }) => {
    const [writer] = await Promise.all([
      page.waitForEvent("popup"),
      page.evaluate(() => window.open(location.href, "_blank")),
    ]);
    await mountLayout(page, '<div id="update-panel"></div>', { basePath });
    await page.evaluate(() => {
      const NativeChannel = window.BroadcastChannel;
      window.__heldCacheAnnouncements = [];
      window.BroadcastChannel = class extends NativeChannel {
        constructor(name) {
          super(name);
          super.onmessage = (event) => {
            if (name === "build-cache") window.__heldCacheAnnouncements.push(() => this.listener?.(event));
            else this.listener?.(event);
          };
        }
        set onmessage(listener) { this.listener = listener; }
        get onmessage() { return this.listener; }
      };
    });
    await loadBrowserModules(page, {
      panel: "src/core/bridgeUpdatePanel.js",
      updates: "src/core/bridgeUpdates.js",
      cache: "src/core/localCache.js",
    }, basePath);
    await loadBrowserModules(writer, { cache: "src/core/localCache.js" }, basePath);
    await page.evaluate((status) => {
      const callRpc = status
        ? (method) => method === "bridge.update_status" ? Promise.resolve(status)
          : new Promise((resolve) => { window.__heldAction = { resolve }; })
        : () => new Promise(() => {});
      window.__layoutModules.panel.mountBridgeUpdatePanel(document.querySelector("#update-panel"), {
        deviceId: "laptop", callRpc,
      });
    }, actionStatus);
    await check({ page, writer });
  });
}

it("keeps a cross-tab failure after a delayed install action reply", async () => {
  await withTwoTabs(async ({ page, writer }) => {
    await page.waitForFunction(() => document.querySelector(".bridge-update-state")?.textContent === "Version 0.3.0 is available.");
    await page.locator("[data-bridge-install-idle]").click();
    await page.waitForFunction(() => Boolean(window.__heldAction));
    await writer.evaluate(async (status) => {
      await window.__layoutModules.cache.writeCached(
        { deviceId: "laptop", entityId: "", kind: "bridge-update" }, status,
      );
    }, FAILED);
    await page.waitForFunction(() => window.__heldCacheAnnouncements.length > 0);
    await page.evaluate((status) => window.__heldAction.resolve(status), QUEUED);
    await page.waitForFunction(() => document.querySelector(".bridge-update-request")?.textContent === "Update request accepted.");
    const current = await page.evaluate(async () => {
      const { cache, updates } = window.__layoutModules;
      return {
        cached: (await cache.readCached(updates.bridgeUpdateAddress("laptop")))?.value,
        state: document.querySelector(".bridge-update-state")?.textContent,
        error: document.querySelector(".bridge-update-error")?.textContent,
      };
    });
    expect(current).toEqual({ cached: FAILED, state: "The update failed.", error: FAILED.last_error });
    await page.evaluate(() => window.__heldCacheAnnouncements.splice(0).forEach((deliver) => deliver()));
  }, { actionStatus: AVAILABLE });
}, 30_000);

for (const { label, newer, reply, expectedState } of [
  { label: "a successful status reply", newer: QUEUED, reply: AVAILABLE, expectedState: "Queued until agents are done." },
  { label: "an unknown-method rejection", newer: FAILED, reply: null, expectedState: "The update failed." },
]) {
  it(`keeps a newer cross-tab update after ${label}`, async () => {
    await withTwoTabs(async ({ page, writer }) => {
      await page.evaluate(() => {
        const { updates } = window.__layoutModules;
        window.__statusRequest = updates.refreshBridgeUpdateStatus("laptop", () => new Promise((resolve, reject) => {
          window.__heldReply = { resolve, reject };
        }));
      });
      await page.waitForFunction(() => Boolean(window.__heldReply));
      await writer.evaluate(async (status) => {
        const { cache } = window.__layoutModules;
        await cache.writeCached({ deviceId: "laptop", entityId: "", kind: "bridge-update" }, status);
      }, newer);
      await page.waitForFunction(() => window.__heldCacheAnnouncements.length > 0);

      await page.evaluate(async (answer) => {
        if (answer) window.__heldReply.resolve(answer);
        else window.__heldReply.reject(new Error("unknown method: bridge.update_status"));
        await window.__statusRequest;
      }, reply);

      const current = await page.evaluate(async () => {
        const { cache, updates } = window.__layoutModules;
        return {
          cached: (await cache.readCached(updates.bridgeUpdateAddress("laptop")))?.value,
          state: document.querySelector(".bridge-update-state")?.textContent,
          error: document.querySelector(".bridge-update-error")?.textContent || null,
        };
      });
      expect(current.cached).toEqual(newer);
      expect(current.state).toBe(expectedState);
      expect(current.error).toBe(newer.last_error);
      await page.evaluate(() => window.__heldCacheAnnouncements.splice(0).forEach((deliver) => deliver()));
    });
  }, 30_000);
}

it("keeps update controls usable when IndexedDB is unavailable", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div id="update-panel"></div>', { basePath });
    await page.evaluate(() => Object.defineProperty(window, "indexedDB", { value: undefined }));
    await loadBrowserModules(page, { panel: "src/core/bridgeUpdatePanel.js" }, basePath);
    await page.evaluate(({ available, queued }) => {
      window.__layoutModules.panel.mountBridgeUpdatePanel(document.querySelector("#update-panel"), {
        deviceId: "laptop",
        callRpc: (method) => Promise.resolve(method === "bridge.update_status" ? available : queued),
      });
    }, { available: AVAILABLE, queued: QUEUED });
    await page.waitForFunction(() => document.querySelector(".bridge-update-state")?.textContent === "Version 0.3.0 is available.");
    await page.locator("[data-bridge-install-idle]").click();
    await page.waitForFunction(() => document.querySelector(".bridge-update-state")?.textContent === "Queued until agents are done.");
  });
}, 30_000);

it("rejects an old action reply after eviction and a new failure", async () => {
  await withTwoTabs(async ({ page, writer }) => {
    await page.waitForFunction(() => document.querySelector(".bridge-update-state")?.textContent === "Version 0.3.0 is available.");
    await page.locator("[data-bridge-install-idle]").click();
    await page.waitForFunction(() => Boolean(window.__heldAction));
    await writer.evaluate(async (status) => {
      const { cache } = window.__layoutModules;
      await cache.evictEntity("laptop", "");
      await cache.writeCached({ deviceId: "laptop", entityId: "", kind: "bridge-update" }, status);
    }, FAILED);
    await page.waitForFunction(() => window.__heldCacheAnnouncements.length > 0);
    await page.evaluate((status) => window.__heldAction.resolve(status), QUEUED);
    await page.waitForFunction(() => document.querySelector(".bridge-update-request")?.textContent === "Update request accepted.");
    const current = await page.evaluate(async () => ({
      cached: (await window.__layoutModules.cache.readCached(
        window.__layoutModules.updates.bridgeUpdateAddress("laptop")))?.value,
      state: document.querySelector(".bridge-update-state")?.textContent,
    }));
    expect(current).toEqual({ cached: FAILED, state: "The update failed." });
  }, { actionStatus: AVAILABLE });
}, 30_000);

it("rejects an old reply after an absent record is written and deleted", async () => {
  await withTwoTabs(async ({ page, writer }) => {
    await page.evaluate(() => {
      window.__statusRequest = window.__layoutModules.updates.refreshBridgeUpdateStatus(
        "laptop", () => new Promise((resolve) => { window.__heldReply = { resolve }; }),
      );
    });
    await page.waitForFunction(() => Boolean(window.__heldReply));
    await writer.evaluate(async (status) => {
      const { cache } = window.__layoutModules;
      const address = { deviceId: "laptop", entityId: "", kind: "bridge-update" };
      await cache.writeCached(address, status);
      await cache.deleteCached([address]);
    }, FAILED);
    await page.waitForFunction(() => window.__heldCacheAnnouncements.length > 0);
    await page.evaluate(async (status) => {
      window.__heldReply.resolve(status);
      await window.__statusRequest;
    }, AVAILABLE);
    const current = await page.evaluate(async () => ({
      cached: await window.__layoutModules.cache.readCached(
        window.__layoutModules.updates.bridgeUpdateAddress("laptop")),
      staleAvailable: document.querySelector(".bridge-update-body")?.textContent.includes("Version 0.3.0 is available"),
    }));
    expect(current).toEqual({ cached: undefined, staleAvailable: false });
  });
}, 30_000);
