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

import { cachedAddresses, cachedAddressesWrittenBefore, cachedRecords, deleteCached, writeCached } from "./localCache.js";

export const WORKSPACE_DATA_TTL_MS = 72 * 60 * 60 * 1000;

/** File bodies kept per workspace, newest opened first. */
export const RECENT_FILES = 5;

/** The largest file body worth keeping. Applied by `cacheFileBody` below,
 *  which is the only way a file body gets into the store. */
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
export async function expireWorkspaceData(deviceId, entityId, now = Date.now(), active = () => true) {
  const stale = await cachedAddressesWrittenBefore({ deviceId, entityId }, now - WORKSPACE_DATA_TTL_MS);
  if (!active()) return [];
  return drop(workspaceData(stale));
}

/** Let go of one workspace's data at once — it is done, or deleted. The feed
 *  row stays: the board is what removes a row. */
export async function evictWorkspaceData(deviceId, entityId, active = () => true) {
  const addresses = await cachedAddresses({ deviceId, entityId });
  if (!active()) return [];
  return drop(workspaceData(addresses));
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

/** How a body is measured against a cap: in bytes on the wire, never in
 *  characters. A source file of Japanese is three bytes a character, so a
 *  cap read off `length` lets three times the rule onto the disk. Nothing to
 *  measure is within every cap.
 *
 *  A body whose character count alone is over the cap is answered without
 *  encoding it: UTF-8 is never shorter than the UTF-16 length, so that
 *  comparison is already decisive, and the encode it saves is the one that
 *  would have allocated the oversized copy this check exists to refuse. */
export function withinBytes(text, maxBytes) {
  if (text === null || text === undefined) return true;
  const body = String(text);
  if (body.length > maxBytes) return false;
  return new TextEncoder().encode(body).length <= maxBytes;
}

/** Whether a read answer is one the cache may keep.
 *
 *  Measured against the file's own size — the bytes on disk, which is what the
 *  rule is about — rather than against the body on the wire, which is base64
 *  and a third bigger than them. A read that came back truncated is refused
 *  whatever it weighs: it is not the file, and a reader opening it again
 *  would be shown a piece of one with nothing saying so. */
const fileBodyFits = (file) => {
  if (!file || file.truncated) return false;
  // `Number(null)` is zero and passes every cap, so the field has to be a
  // number before it is read as one: an answer that names no size is measured
  // by what it carries.
  const sized = typeof file.size === "number" && Number.isFinite(file.size);
  return sized ? file.size <= FILE_MAX_BYTES : withinBytes(file.content_b64, FILE_MAX_BYTES);
};

/** Put one file's body in the cache, under both of the owner's rules for it:
 *  a body over `FILE_MAX_BYTES` is not stored at all, and a write leaves at
 *  most `RECENT_FILES` bodies behind it. `file` is the `fs.read` answer as it
 *  came. Answers whether the body was stored.
 *
 *  Every file body goes in through here. A writer that wrote the record
 *  itself would be a second place the two rules have to be remembered, and
 *  the one that forgot them would put a 40 MB body under a 1 MB row of the
 *  Records table.
 *
 *  A refusal is not an error: the reader gets the file off the wire as they
 *  always would, and the only thing lost is the instant second look. */
export async function cacheFileBody({ deviceId, entityId, path, file, openedAt = Date.now() }) {
  if (!deviceId || !entityId || !fileBodyFits(file)) return false;
  await writeCached({ deviceId, entityId, kind: FILE_RECORD_KIND, sub: path || "" }, { file, openedAt });
  await trimRecentFiles(deviceId, entityId);
  return true;
}
