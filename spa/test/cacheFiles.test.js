// @vitest-environment jsdom
// The Files tab against the local cache: a saved directory listing paints
// while the machine is being asked, the live answer replaces and rewrites it,
// and a machine that cannot answer leaves the saved listing standing.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { setCacheDevice } = await import("../src/core/cacheScope.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { renderFilesTab } = await import("../src/views/files.js");

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

const treeNames = (host) =>
  [...host.querySelectorAll(".frow")].map((row) => (row.dataset.dir || row.dataset.file || "").trim());

beforeEach(async () => {
  document.body.innerHTML = "";
  setCacheDevice("dev-1");
  await wipeCache();
});

const mountFiles = (callRpc) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, { scope: { run_id: "run-1" }, callRpc });
  return { host, files };
};

describe("the cached listing", () => {
  it("paints the saved directory while the machine is being asked", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" },
      { path: "", entries: [{ name: "src", kind: "dir" }, { name: "README.md", kind: "file", size: 12 }] },
    );
    const { host } = mountFiles(vi.fn(() => new Promise(() => {})));
    await settle();
    expect(treeNames(host)).toEqual(["src", "README.md"]);
  });

  it("lets the live answer replace the saved one and writes it through", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" },
      { path: "", entries: [{ name: "stale.js", kind: "file", size: 1 }] },
    );
    const { host } = mountFiles(
      vi.fn(async () => ({ path: "", entries: [{ name: "fresh.js", kind: "file", size: 2 }] })),
    );
    await settle();
    expect(treeNames(host)).toEqual(["fresh.js"]);
    const record = await readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" });
    expect(record.value.entries[0].name).toBe("fresh.js");
  });

  it("keeps the saved listing when the machine cannot answer", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "" },
      { path: "", entries: [{ name: "kept.js", kind: "file", size: 1 }] },
    );
    const { host } = mountFiles(vi.fn(async () => Promise.reject(new Error("unreachable"))));
    await settle();
    expect(treeNames(host)).toEqual(["kept.js"]);
    expect(host.textContent).not.toContain("cannot list");
  });
});
