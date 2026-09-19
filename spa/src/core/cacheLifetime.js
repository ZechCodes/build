// How long the cache keeps what a workspace holds (the owner's rule, 2026-09-18).
//
// A workspace's data — its status, commits, diffs, trees, terminals, threads,
// file bodies, the surfaces and the console's tab pick — is worth holding for
// as long as the reader might come back to it, and worth nothing once they
// cannot. So:
//
//   finished or deleted   everything goes at once, the moment the board says so
//   only recent           it ages out 72 h after its last write
//   active                it never expires
//
// The one thing that stays is the feed row: it is the board's to list and the
// board's to remove, and a workspace with no data still has a line on the
// inbox until the feed stops naming it. Nothing here decides which workspaces
// are active or recent — the caller knows that, and calls accordingly.

import { cachedAddresses, cachedAddressesWrittenBefore, cachedRecords, deleteCached } from "./localCache.js";

export const WORKSPACE_DATA_TTL_MS = 72 * 60 * 60 * 1000;

/** File bodies kept per workspace, newest opened first. */
export const RECENT_FILES = 5;

/** The largest file body worth keeping. Nothing here enforces it — a cap on
 *  what goes in belongs to whoever writes the record, and this module only
 *  decides what comes out. It lives here so the writer has one place to read
 *  it from, beside the count it goes with. */
export const FILE_MAX_BYTES = 1048576;

export const FILE_RECORD_KIND = "file";

/** The kinds under a workspace that are not its data. Written as what is kept
 *  rather than what goes: a kind added later that nobody thought to list here
 *  should expire with the workspace, not outlive it forever. */
const KEPT_KINDS = new Set(["row"]);

export const isWorkspaceDataKind = (kind) => !KEPT_KINDS.has(kind);

const workspaceData = (addresses) => addresses.filter((address) => isWorkspaceDataKind(address.kind));

/** Drop these addresses, and answer them. */
const drop = async (addresses) => {
  await deleteCached(addresses);
  return addresses;
};

/** Age out one recent workspace's data: everything last written more than the
 *  TTL ago. Called for a workspace the board still lists but nobody is on.
 *
 *  The read and the delete are two transactions, so a record rewritten in
 *  between is dropped on the age it had when the sweep started. That costs
 *  the reader a cold read of something just synced, which the next sync pass
 *  refills — the other way round, holding a readwrite transaction open over
 *  the whole sweep, would block the writers this cache exists to serve. */
export async function expireWorkspaceData(deviceId, entityId, now = Date.now()) {
  const stale = await cachedAddressesWrittenBefore({ deviceId, entityId }, now - WORKSPACE_DATA_TTL_MS);
  return drop(workspaceData(stale));
}

/** Let go of one workspace's data at once — it is done, or deleted. The feed
 *  row stays: the board is what removes a row. */
export async function evictWorkspaceData(deviceId, entityId) {
  return drop(workspaceData(await cachedAddresses({ deviceId, entityId })));
}

/** When a file body was last opened — the writer stamps it; a record written
 *  without one counts as opened when it was written. */
const openedAt = (record) =>
  typeof record.value?.openedAt === "number" ? record.value.openedAt : record.at;

/** Keep one workspace's five most recently opened file bodies and drop the
 *  rest. Called after a file is read into the cache. */
export async function trimRecentFiles(deviceId, entityId) {
  const files = await cachedRecords({ deviceId, entityId, kind: FILE_RECORD_KIND });
  if (files.length <= RECENT_FILES) return [];
  const newestFirst = [...files].sort((one, other) => openedAt(other) - openedAt(one));
  return drop(newestFirst.slice(RECENT_FILES).map((record) => record.address));
}
