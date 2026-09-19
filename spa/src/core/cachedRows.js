// One device's rows, out of the cache, and which of them a route stands on.
//
// A route names a workspace, a branch or an issue; a record is addressed by an
// ENTITY, and the two are not the same id — a workspace's entity is the
// conversation it holds. Turning one into the other takes the device's rows and
// the two lists a workspace route is named by, so both readers of that question
// (the sync layer choosing which workspace to watch in realtime, and a view
// finding its own row) ask it here rather than each keeping a copy.
//
// Off the records rather than off the `feed` the last pass wrote, because a
// workspace created since that pass rode in on its own `state` item — the board
// pushes deltas, not the whole board — and lives in the cache as a row and
// nowhere else. That workspace is the likeliest of all to be the one being
// stood on: the reader just made it and walked in.

import { cachedAddresses, readCached, readCachedMany } from "./localCache.js";
import { routedEntry } from "./inbox.js";

export const ROW_RECORD_KIND = "row";

/** One device's world as a route is resolved against it. */
export async function cachedFeedView(deviceId) {
  if (!deviceId) return { items: [], projects: [], workspaces: [] };
  const rowAddresses = (await cachedAddresses({ deviceId })).filter((address) => address.kind === ROW_RECORD_KIND);
  const [rows, projects, workspaces] = await Promise.all([
    readCachedMany(rowAddresses),
    readCached({ deviceId, entityId: "", kind: "projects" }),
    readCached({ deviceId, entityId: "", kind: "workspaces" }),
  ]);
  return {
    items: rows.map((record) => record?.value).filter(Boolean),
    projects: projects?.value || [],
    workspaces: workspaces?.value || [],
  };
}

/** The row this route is standing on, on this device — or null while the cache
 *  holds none that answers to it. */
export async function cachedRouteEntry(deviceId, route) {
  return routedEntry(route, await cachedFeedView(deviceId));
}

/** The entity this route is standing on, on this device. */
export async function cachedRouteEntityId(deviceId, route) {
  return (await cachedRouteEntry(deviceId, route))?.entityId || null;
}
