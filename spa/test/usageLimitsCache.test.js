// @vitest-environment jsdom
// The render boundary for a device's usage limits is the local-cache record:
// a mounted banner reads that record before any board payload arrives, and a
// later write announces only its address so the banner must re-read to paint.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const heldLimit = {
  harness: "claude_adk",
  since: "2026-09-20T21:30:47Z",
  resets_at: "2026-09-20T22:20:00Z",
  said: "You've hit your session limit · resets 6:20pm (America/New_York)",
};

let cache;
let limits;

const panel = () => document.querySelector("#rail-panel");

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  document.body.innerHTML = `
    <div id="rail-panel">
      <div class="rail-head"></div>
      <div class="rail-body" id="rail-body"></div>
    </div>`;
  cache = await import("../src/core/localCache.js");
  limits = await import("../src/core/usageLimits.js");
});

describe("the cached usage-limit render path", () => {
  it("paints a cached limit without waiting for a board payload", async () => {
    await cache.writeCached(limits.usageLimitsAddress("dev-1"), [heldLimit]);
    await limits.readCachedUsageLimits("dev-1");
    limits.resetUsageLimits();

    const banner = limits.mountUsageLimitBanner(panel, "dev-1");

    await vi.waitFor(() => {
      expect(panel().querySelector(".usage-limit-banner")?.textContent).toContain(
        "Claude session limit reached",
      );
    });
    banner.dispose();
  });

  it("redraws through a real cache write, announcement, and readback", async () => {
    const banner = limits.mountUsageLimitBanner(panel, "dev-1");
    expect(panel().querySelector(".usage-limit-banner")).toBeNull();

    await cache.writeCached(limits.usageLimitsAddress("dev-1"), [heldLimit]);

    await vi.waitFor(() => {
      expect(limits.usageLimitsOf("dev-1")).toEqual([heldLimit]);
      expect(panel().querySelector(".usage-limit-banner")?.textContent).toContain(
        "Claude session limit reached",
      );
    });

    await cache.writeCached(limits.usageLimitsAddress("dev-1"), []);
    await vi.waitFor(() => {
      expect(limits.usageLimitsOf("dev-1")).toEqual([]);
      expect(panel().querySelector(".usage-limit-banner")).toBeNull();
    });
    banner.dispose();
  });
});
