// @vitest-environment jsdom
// The All-changes review plug against the local cache: a saved aggregate diff
// paints read-only while the live one is fetched, and every changed live
// paint writes through for the next visit.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { scopeFor } = await import("../src/core/cacheScope.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { createReviewPlug } = await import("../src/core/changesReview.js");
const { resetChangeEvents } = await import("../src/core/changeEvents.js");

const PATCH = `diff --git a/src/a.js b/src/a.js
index 1111111..2222222 100644
--- a/src/a.js
+++ b/src/a.js
@@ -1,2 +1,2 @@
-old
+cached line
`;

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

let host, plug;

beforeEach(async () => {
  document.body.innerHTML = "";
  resetChangeEvents();
  await wipeCache();
  host = document.createElement("div");
  document.body.appendChild(host);
  plug = null;
});

describe("the saved aggregate diff", () => {
  /** A plug as a surface makes one: the cache of the machine the diff is on,
   *  handed down, and whatever else the case in hand wants. */
  const plugOn = (deviceId, options) => createReviewPlug({ cacheScope: scopeFor(deviceId), ...options });

  it("tracks the files actually visible in the all-changes scroller", async () => {
    const viewingContext = { setVisibleDiffs: vi.fn(), captureDomSelection: vi.fn(), clearSelection: vi.fn(), clear: vi.fn() };
    plug = plugOn("dev-1", { fetchDiff: vi.fn(async () => ({ patch: PATCH })), entity: "run-1", viewingContext });
    plug.mount(host);
    await settle();
    expect(viewingContext.setVisibleDiffs).toHaveBeenCalledWith(host, "all");
    viewingContext.setVisibleDiffs.mockClear();
    host.dispatchEvent(new Event("scroll"));
    await settle();
    expect(viewingContext.setVisibleDiffs).toHaveBeenCalledWith(host, "all");
    plug.unmount();
  });

  it("paints whole — tray included — while the live one is being fetched", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH, commentable: true });
    plug = plugOn("dev-1", { fetchDiff: vi.fn(() => new Promise(() => {})), entity: "run-1", submit: vi.fn() });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("cached line");
    // The chrome is not a round trip late: the comment affordances the last
    // live paint had are on the cached stack too.
    expect(host.querySelector(".fcmt")).toBeTruthy();
    plug.unmount();
  });

  it("stays read-only when the last live paint said so", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH, commentable: false });
    plug = plugOn("dev-1", { fetchDiff: vi.fn(() => new Promise(() => {})), entity: "run-1", submit: vi.fn() });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("cached line");
    expect(host.querySelector(".fcmt")).toBeNull();
    plug.unmount();
  });

  it("lets the live diff replace it and writes the change through", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH });
    const live = PATCH.replace("cached line", "live line");
    plug = plugOn("dev-1", { fetchDiff: vi.fn(async () => ({ patch: live, key: "building" })), entity: "run-1" });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("live line");
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" });
    expect(record.value.patch).toBe(live);
    plug.unmount();
  });

  // The plug is made for one machine's diff: the scope the surface handed it is
  // what it is filed under, whatever machine the rest of this suite works.
  it("caches under the cacheScope it is handed", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH });
    const live = PATCH.replace("cached line", "their line");
    plug = plugOn("dev-2", { fetchDiff: vi.fn(async () => ({ patch: live })), entity: "run-1" });
    plug.mount(host);
    await settle();

    expect((await readCached({ deviceId: "dev-2", entityId: "run-1", kind: "diff" })).value.patch).toBe(live);
    expect((await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" })).value.patch).toBe(PATCH);
    plug.unmount();
  });

  it("caches nothing for a surface that names no entity", async () => {
    plug = plugOn("dev-1", { fetchDiff: vi.fn(async () => ({ patch: PATCH })), entity: null });
    plug.mount(host);
    await settle();
    expect(await readCached({ deviceId: "dev-1", entityId: "", kind: "diff" })).toBeUndefined();
    plug.unmount();
  });
});
