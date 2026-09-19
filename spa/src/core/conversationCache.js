// A conversation, as the disk holds it.
//
// Everything a panel draws of a conversation is in one record — the window of
// items, how far back it reaches, what each activity run totals — and the sync
// layer is what fills it (core/cacheSync.js). This module is the view's end of
// that: it opens the record into the thread cache the panel paints from, opens
// it again when the record moves, and writes the one thing a view puts in there
// itself — a message its reader has just sent, standing in until the wire
// carries it back.
//
// There is no second store. A view that kept the conversation in one place and
// its reader's own message in another would have to merge them on every paint,
// and would still show the two out of order.

import { mergeCached, readCached } from "./localCache.js";
import {
  THREAD_RECORD_KIND,
  acknowledgeProvisionalItem,
  mergeThreadItems,
  provisionalThreadItem,
  withoutProvisionalItem,
} from "./thread.js";
import {
  surfacesCacheAddress,
  surfacesFromRecord,
  surfaceSessionGeneration,
} from "./surfacesCache.js";

export const threadCacheAddress = ({ deviceId, entityId, agentId, conversationId }) => ({
  deviceId,
  // The workspace is the entity a transcript is stored under, always: that
  // prefix is what Done, Delete and the 72 h expiry sweep, and a record
  // addressed outside it would outlive the workspace it belongs to for ever.
  entityId,
  kind: THREAD_RECORD_KIND,
  // A conversation is the transcript's canonical storage owner within the
  // workspace. Issue and run views may intentionally point at the same one;
  // their agent ids must not fork that history into two browser caches.
  sub: conversationId || agentId || "",
});

/** The shape a window has before anything has been read into it — what the
 *  first message in a brand-new conversation is written into. */
const EMPTY_WINDOW = Object.freeze({
  items: [],
  olderItemsRemain: false,
  deliveredSequence: 0,
  knownTotalItems: null,
  activityDigests: [],
});

const withItems = (held, items) => ({ ...EMPTY_WINDOW, ...held, items });

/** Put a message the reader has just sent into the conversation, under the
 *  operation carrying it. Merged under the address, because the sync layer is
 *  writing the same record from the other side. */
export const writeProvisionalMessage = (address, operationId, message) =>
  mergeCached(address, (held) =>
    withItems(held, mergeThreadItems(held?.items || [], [provisionalThreadItem({ operationId, message })])));

/** The post was taken: stamp the sequence it was written at onto the message
 *  waiting for it, so it sits where the conversation will put it. */
export const acknowledgeProvisionalMessage = (address, operationId, sequence, deliveryStatus) =>
  mergeCached(address, (held) => {
    const items = acknowledgeProvisionalItem(held?.items || [], operationId, sequence, deliveryStatus);
    return held && items !== held.items ? withItems(held, items) : null;
  });

/** The post was refused: take the message back out. */
export const withdrawProvisionalMessage = (address, operationId) =>
  mergeCached(address, (held) => {
    const items = withoutProvisionalItem(held?.items || [], operationId);
    return held && items !== held.items ? withItems(held, items) : null;
  });

export function createConversationCache({ addressOf, threadCache, onThreadSeeded, onSurfacesSeeded }) {
  let opened = false;
  let surfacesSeeded = false;

  /** Whether the panel is still on the conversation a read was made for. A
   *  window opened under the reader after they pressed another bubble would
   *  draw one agent's words under another's name. */
  const sameConversation = (captured, current) =>
    !!current && current.entityId === captured.entityId && current.agentId === captured.agentId;

  const sameSurfaceIdentity = (captured, current) =>
    sameConversation(captured, current)
    && current.deviceId === captured.deviceId
    && surfaceSessionGeneration(current.surfaceSessionGeneration)
      === surfaceSessionGeneration(captured.surfaceSessionGeneration);

  const seedSurfaces = (record, identity) => {
    const seen = surfacesFromRecord(record, surfaceSessionGeneration(identity.surfaceSessionGeneration));
    if (!seen || surfacesSeeded) return;
    surfacesSeeded = true;
    onSurfacesSeeded(seen);
  };

  /** The record IS the conversation: what it holds replaces what the panel was
   *  drawn from, however far the reader had widened it. Their own widening is
   *  written back to the record (core/agentRailContext.js), so the record is
   *  always the wider of the two. */
  const openWindow = (record, identity) => {
    opened = true;
    threadCache.seedWindow(record?.value || null);
    onThreadSeeded(identity.agentId);
  };

  return {
    /** Where this conversation is held, for whoever wants to hear it move. */
    address() {
      const identity = addressOf();
      return identity ? threadCacheAddress(identity) : null;
    },

    /** Open what the disk holds: the conversation's window, and the surfaces
     *  the last live session observed. Once per conversation. */
    async seed() {
      const identity = addressOf();
      if (!identity || opened) return;
      opened = true;
      const [thread, surfaces] = await Promise.all([
        readCached(threadCacheAddress(identity)),
        readCached(surfacesCacheAddress(identity)),
      ]);
      const stillOpen = addressOf();
      if (!sameConversation(identity, stillOpen)) return;
      if (sameSurfaceIdentity(identity, stillOpen)) seedSurfaces(surfaces, identity);
      openWindow(thread, identity);
    },

    /** The window again, after something wrote to the record. */
    async reread() {
      const identity = addressOf();
      if (!identity) return;
      const thread = await readCached(threadCacheAddress(identity));
      if (!sameConversation(identity, addressOf())) return;
      openWindow(thread, identity);
    },

    reset() {
      threadCache.reset();
      opened = false;
      surfacesSeeded = false;
    },
  };
}
