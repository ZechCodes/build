// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const fake = vi.hoisted(() => ({ lazy: false, greeted: new Set(), watchers: [] }));
vi.mock("../src/core/scopedSyncRuntime.js", async (original) => ({ ...await original(), lazyViewReadsEnabled: () => fake.lazy }));
vi.mock("../src/core/changeEvents.js", async (original) => ({ ...await original(),
  bridgeCapabilities: () => ({ changes: { filesHints: true }, bodies: {}, diffs: {} }),
  onBridgeGreeted: (listener) => { fake.greeted.add(listener); return () => fake.greeted.delete(listener); },
  watchChanges: (options) => {
    const held = { ...options, disposed: false }; fake.watchers.push(held);
    return { dispose: () => { held.disposed = true; } };
  },
}));
let pane, host;
beforeEach(async () => {
  vi.resetModules(); fake.lazy = false; fake.greeted.clear(); fake.watchers = [];
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  host = document.createElement("div"); document.body.appendChild(host);
});
afterEach(() => { pane?.dispose(); host.remove(); });

it("acquires and releases a visible Git watcher when a cached mount changes sync ownership after greeting", async () => {
  const deviceId = "git-late-greeting";
  const { writeCached } = await import("../src/core/localCache.js");
  const { scopeFor } = await import("../src/core/cacheScope.js");
  const status = { branch: "dev", status_key: "held", files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } };
  await writeCached({ deviceId, entityId: "run-1", kind: "status" }, status);
  await writeCached({ deviceId, entityId: "run-1", kind: "log" }, { commits: [], more: false });
  const { mountGitPane } = await import("../src/core/gitPane.js");
  pane = mountGitPane(host, { scope: { run_id: "run-1" }, cacheScope: scopeFor(deviceId),
    callRpc: async (method) => method === "git.status" ? status : { commits: [], more: false } });
  await vi.waitFor(() => expect(host.querySelector(".csinput")).not.toBeNull());
  expect(fake.watchers.filter((watcher) => !watcher.disposed && watcher.kinds?.includes("git"))).toHaveLength(0);
  fake.lazy = true;
  for (const listener of fake.greeted) listener(deviceId);
  const watching = fake.watchers.find((watcher) => !watcher.disposed && watcher.kinds?.includes("git"));
  expect(watching?.kinds).toContain("files.hints");
  fake.lazy = false;
  for (const listener of fake.greeted) listener(deviceId);
  expect(watching.disposed).toBe(true);
});
