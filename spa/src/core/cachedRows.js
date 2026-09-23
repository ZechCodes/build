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

import { cachedAddresses, deleteCached, readCached, readCachedMany, writeCached } from "./localCache.js";
import { entryKeyOf, routedEntry } from "./inbox.js";
import { entityIdOf } from "./entityId.js";
import { withCacheFreshness } from "./cacheFreshness.js";

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
    items: rows.map((record) => withCacheFreshness(record?.value, record)).filter(Boolean),
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

// ─── The optimistic writes over those rows ───────────────────────────────────
//
// A press that moves a row — mute, clear, Done — moves it in the cache and
// lets the `state` push that follows confirm it. The cache is the only thing a
// view reads, so writing there is the whole of showing the move: every surface
// holding the row hears it, and the move survives a remount and a reload.
//
// Both places, because neither is the whole of where a row lives: its own
// record is what a push rewrites, and the board list is the only place a row
// naming no entity is at all.
//
// Each write answers the way to undo it — the values as they stood — for the
// bridge refusing. Restoring rather than re-patching, because the answer to
// "what was it before" is not derivable from the fields that went on.

const FEED_RECORD_ADDRESS = (deviceId) => ({ deviceId, entityId: "", kind: "feed" });

const rowAddress = (deviceId, entityId) => ({ deviceId, entityId, kind: ROW_RECORD_KIND });

/** Whether a board row is the one being moved. By the entity where there is
 *  one — the rail lists a workspace under its own name and the row under the
 *  conversation's, and both stand for the same line — and by the listing key
 *  where there is not, which is the only name a bare checkout has. */
const namesRow = (item, { key, entityId }) =>
  entityId ? entityIdOf(item) === entityId : entryKeyOf(item) === key;

/** The board list with one row rewritten by `rewrite`, or dropped where it
 *  answers null. A list that does not name the row is answered null. */
function boardListWith(held, target, rewrite) {
  const items = held?.items;
  if (!Array.isArray(items)) return null;
  const next = [];
  let moved = false;
  for (const item of items) {
    if (!namesRow(item, target)) {
      next.push(item);
      continue;
    }
    moved = true;
    const rewritten = rewrite(item);
    if (rewritten) next.push(rewritten);
  }
  return moved ? { ...held, items: next } : null;
}

/** Rewrite one row wherever this device holds it, and answer the undo. The
 *  row is named the way the rail names it — the key it is listed under and the
 *  entity it is addressed by, both of which an inbox entry carries. */
async function writeRowEverywhere(deviceId, target, rewrite) {
  const address = target.entityId ? rowAddress(deviceId, target.entityId) : null;
  const heldRow = address ? (await readCached(address))?.value : null;
  const heldFeedRecord = await readCached(FEED_RECORD_ADDRESS(deviceId));
  const heldFeed = heldFeedRecord?.value;
  const board = boardListWith(heldFeed, target, rewrite);
  if (address && heldRow) {
    const rewritten = rewrite(heldRow);
    if (rewritten) await writeCached(address, rewritten);
    else await deleteCached([address]);
  }
  if (board) await writeCached(FEED_RECORD_ADDRESS(deviceId), board);
  return async () => {
    if (address && heldRow) await writeCached(address, heldRow);
    if (board) await writeCached(FEED_RECORD_ADDRESS(deviceId), heldFeed);
  };
}

/** Lay `fields` over one row, in the cache, now. Answers the undo. */
export const patchFeedRow = (deviceId, target, fields) =>
  writeRowEverywhere(deviceId, target, (held) => ({ ...held, ...fields }));

/** Take one row out of the cache, now — the work is over. Answers the undo. */
export const removeFeedRow = (deviceId, target) => writeRowEverywhere(deviceId, target, () => null);

/** How a board row names itself to the two writers above. */
export const feedRowTarget = (row) => ({ key: entryKeyOf(row), entityId: entityIdOf(row) });
