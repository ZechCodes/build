// One device's world as the console reads it: the two lists, the row the route
// stands on, and the shells that row's checkout is holding.
//
// The console paints from the cache and asks the machine nothing to do it, so
// every suite that mounts one has to put a world on the disk first. This is
// that world, written once so no suite invents its own shape for it.

import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { scopeFor } from "../src/core/cacheScope.js";
import { writeCached, wipeCache } from "../src/core/localCache.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

export const consoleBranchRow = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  project_id: "p1",
  projectKey: "dev-1:p1",
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  ...over,
});

const termTabs = (ids) => ids.map((id) => (typeof id === "string" ? { term_id: id } : id));

/** Put one device's rows and one row's terminals where the console looks. */
const entityOf = (row) => row.run_id || row.worktree_id || row.issue_id || row.branch || "";

export async function seedConsoleWorld({ deviceId = "dev-1", row = consoleBranchRow(), terminals = [] } = {}) {
  await writeCached({ deviceId, entityId: "", kind: "projects" }, []);
  await writeCached({ deviceId, entityId: "", kind: "workspaces" }, []);
  if (!row) return;
  await writeCached({ deviceId, entityId: entityOf(row), kind: "row" }, row);
  // `terminals: null` is a checkout nothing has answered for yet — which is not
  // the same world as `terminals: []`, a checkout holding no shells.
  if (terminals) await seedConsoleTerminals(termTabs(terminals), { deviceId, row });
}

/** The tab strip, as a `terminals` push leaves it. */
export async function seedConsoleTerminals(ids, { deviceId = "dev-1", row = consoleBranchRow() } = {}) {
  await writeCached({ deviceId, entityId: entityOf(row), kind: "terminals" }, { tabs: termTabs(ids) });
}

export const emptyConsoleWorld = () => wipeCache();

/** The cache a console mounted for one device writes under. */
export const consoleCacheScope = (deviceId = "dev-1") => scopeFor(deviceId);
