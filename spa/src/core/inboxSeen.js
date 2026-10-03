// The read cursor: telling the daemon an entry has been read, on the machine
// that keeps it.
//
// A caller names an entity and nothing else — the agent rail knows the bubble
// the reader scrolled past, not which bridge answered for it — so the rail's
// rows are indexed by entity id here and the report goes to the device the row
// came from.

import { entityIdOf } from "./entityId.js";
import { verbCall } from "./inboxDevices.js";

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
 * This is also the hook for a self-initiated ending: merge and abandon are
 * attention-class events, so a merge the user triggered from this client would
 * otherwise badge its own entry. Whoever runs that verb calls this after it.
 */
export async function markSeen(entityId, agentId, readFromSequence = null, readThroughSequence = null, threadId = "") {
  if (!entityId) return;
  try {
    await verbCall(rowHolding(entityId))("entity.seen", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      ...(threadId ? { thread_id: threadId } : {}),
      ...(typeof readFromSequence === "number" ? { read_from_sequence: readFromSequence } : {}),
      ...(typeof readThroughSequence === "number" ? { read_through_sequence: readThroughSequence } : {}),
    });
  } catch {
    /* the cursor is the daemon's; a failed clear is re-tried by the next open */
  }
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
