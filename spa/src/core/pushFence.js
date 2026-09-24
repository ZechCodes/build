// What the pushes wrote while a pass was out reading (#142).
//
// A pass holds its subscriptions before it asks for anything
// (core/cacheSync.js), so whatever changes after a read is asked is pushed.
// That closes the gap the bridge leaves — it records a change only for the
// subscriptions it holds when the change happens — and opens a race in its
// place: a push can land before the read it overtook is written, and the read
// is then the older word. So every push marks the record it wrote, a read
// takes the fence before it asks, and a read writes only where no push has
// marked the record since.
//
// Held in memory only, one mark per record a push has touched: a read never
// outlives the tab that asked it, and the sync layer forgets them all when it
// stands down.

let pushes = 0;
const marks = new Map(); // record address → the latest push that touched it

const markKey = ({ deviceId, entityId, kind, sub = "" }) => JSON.stringify([deviceId, entityId, kind, sub]);

/** Where the pushes stand now: what a read takes before it asks. */
export const pushFence = () => pushes;

/** A push wrote this record — or took it away, with the entity it belonged to. */
export function notePush(address, { removed = false } = {}) {
  pushes += 1;
  marks.set(markKey(address), { at: pushes, removed });
}

/** What a push did to this record after the fence, or null where none did. */
export function pushedSince(address, fence) {
  const mark = marks.get(markKey(address));
  return mark && mark.at > fence ? mark : null;
}

/** The sync layer stood down: no read it asked will be written. */
export function forgetPushes() {
  marks.clear();
}
