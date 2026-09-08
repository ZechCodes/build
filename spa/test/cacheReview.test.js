// @vitest-environment jsdom
// The All-changes review plug against the local cache: a saved aggregate diff
// paints read-only while the live one is fetched, and every changed live
// paint writes through for the next visit.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { setCacheDevice } = await import("../src/core/cacheScope.js");
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
  setCacheDevice("dev-1");
  await wipeCache();
  host = document.createElement("div");
  document.body.appendChild(host);
  plug = null;
});

describe("the saved aggregate diff", () => {
  it("paints whole — tray included — while the live one is being fetched", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH, commentable: true });
    plug = createReviewPlug({ fetchDiff: vi.fn(() => new Promise(() => {})), entity: "run-1", submit: vi.fn() });
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
    plug = createReviewPlug({ fetchDiff: vi.fn(() => new Promise(() => {})), entity: "run-1", submit: vi.fn() });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("cached line");
    expect(host.querySelector(".fcmt")).toBeNull();
    plug.unmount();
  });

  it("lets the live diff replace it and writes the change through", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" }, { patch: PATCH });
    const live = PATCH.replace("cached line", "live line");
    plug = createReviewPlug({ fetchDiff: vi.fn(async () => ({ patch: live, key: "building" })), entity: "run-1" });
    plug.mount(host);
    await settle();
    expect(host.textContent).toContain("live line");
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "diff" });
    expect(record.value.patch).toBe(live);
    plug.unmount();
  });

  it("caches nothing for a surface that names no entity", async () => {
    plug = createReviewPlug({ fetchDiff: vi.fn(async () => ({ patch: PATCH })), entity: null });
    plug.mount(host);
    await settle();
    expect(await readCached({ deviceId: "dev-1", entityId: "", kind: "diff" })).toBeUndefined();
    plug.unmount();
  });
});
