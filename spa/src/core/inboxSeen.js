// The read cursor: telling the daemon an entry has been read, on the machine
// that keeps it.
//
// An inbox caller names an entity, so its row supplies the device. A chat
// already knows its device and supplies it explicitly, even before the inbox
// has indexed that owner's row.

import { entityIdOf } from "./entityId.js";
import { verbCall } from "./inboxDevices.js";
import { homeContext } from "./deviceContexts.js";
import { stampRow } from "./feedMerge.js";
import { cachedWriteOf, captureCachedRecord, mergeCachedIfUnwritten } from "./localCache.js";

// The row holding each entity the feed named, minted with every snapshot.
let rowsByEntity = new Map();

/**
 * Tell the bridge this entry has been read. No agent id means the whole entry —
 * which is what opening it means; a bubble passes its own agent.
 *
 * A reader who holds a WINDOW on a long conversation rather than the whole of
 * it passes the sequence that window starts at, so the daemon moves the read
 * cursor only as far as the reader was actually sent. No floor says what it
 * always said: the conversation arrived whole.
 *
 * `readThroughSequence` is the newest message the reader's viewport actually
 * reached. Reading is per message — a panel showing half of what arrived clears
 * half of it — and no sequence says the reader read to the end of what they
 * hold, which is what opening a whole entry means.
 *
 * A chat passes its device explicitly so a cold inbox cannot route its read
 * through another machine. Returns true only after the bridge confirms it.
 *
 * This is also the hook for a self-initiated ending: merge and abandon are
 * attention-class events, so a merge the user triggered from this client would
 * otherwise badge its own entry. Whoever runs that verb calls this after it.
 */
export async function markSeen(entityId, agentId, readFromSequence = null, readThroughSequence = null, threadId = "", deviceId = null) {
  if (!entityId) return false;
  const row = seenRow(entityId, deviceId);
  const call = verbCall(row);
  try {
    await call("entity.seen", seenParams(entityId, agentId, readFromSequence, readThroughSequence, threadId));
  } catch {
    return false;
  }
  // The mark is confirmed even if this compatibility read cannot finish.
  await refreshReadRoster(call, seenDeviceId(row), entityId).catch(() => null);
  return true;
}

const seenRow = (entityId, deviceId) => deviceId ? { ...rowHolding(entityId), deviceId } : rowHolding(entityId);
const seenDeviceId = (row) => row?.deviceId || homeContext()?.deviceId;
const seenParams = (entityId, agentId, floor, read, threadId) => ({
  entity_id: entityId,
  ...(agentId ? { agent_id: agentId } : {}),
  ...(threadId ? { thread_id: threadId } : {}),
  ...(typeof floor === "number" ? { read_from_sequence: floor } : {}),
  ...(typeof read === "number" ? { read_through_sequence: read } : {}),
});

const rosterIdentity = (row) => JSON.stringify((row.agents || []).map((agent) =>
  [agent.id, agent.conversation_id, agent.thread_id]));

/** Older bridges emit only a board revision after a read. Refresh the exact
 *  cached roster the badges use, including owners absent from inbox items.
 *  An intervening cache write or generation change always wins. */
async function refreshReadRoster(call, deviceId, entityId) {
  if (!deviceId) return;
  const address = { deviceId, entityId, kind: "row", sub: "" };
  const before = await captureCachedRecord(address);
  if (!before?.value) return;
  const board = await call("board.list", {});
  const rows = [...(board.items || []), ...(board.runs || []), ...(board.plans || []), ...(board.external_worktrees || [])];
  const row = rows.find((candidate) => entityIdOf(candidate) === entityId);
  if (!row) return;
  await mergeCachedIfUnwritten(address, cachedWriteOf(before), (held) =>
    held && rosterIdentity(held) === rosterIdentity(row) ? { ...held, ...stampRow(row, deviceId) } : null);
}

/** The row holding an entity, off the index the last snapshot minted. A caller
 *  names an entity and nothing else — only the feed knows which machine
 *  answered for it, and that is the machine the read cursor belongs to. An
 *  entity no row names is nobody's in particular, and its cursor goes home
 *  (core/inboxDevices.js). */
const rowHolding = (entityId) => rowsByEntity.get(entityId) || null;

/** Every row that names an entity, by that id — minted with each snapshot
 *  rather than scanned for: the agent rail reports a read on every bubble the
 *  reader scrolls past. */
export function indexRowsByEntity(rows) {
  rowsByEntity = new Map();
  for (const row of rows) {
    const entityId = entityIdOf(row);
    if (entityId && !rowsByEntity.has(entityId)) rowsByEntity.set(entityId, row);
  }
}

/** The entries a mutation from this client just ended, cleared in one call. */
export function noteSelfAction(...entityIds) {
  return Promise.all([...new Set(entityIds.filter(Boolean))].map((id) => markSeen(id)));
}
