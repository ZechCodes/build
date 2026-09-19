// The divider belongs to the oldest unread agent message, never to a user
// message or an activity event that happens to follow the read cursor.
const sequenceOf = (item) => Number(item?.data?.sequence ?? NaN);
export const isAgentMessage = (item) => item?.type === "message" && item.data?.role === "agent";
const agentMessages = (items = []) => items.filter(isAgentMessage);

export function unreadAnchorSequence({ cursor, held, items = [], unreadCount } = {}) {
  if (!Number.isFinite(cursor)) return null;
  const messages = agentMessages(items);
  if (held != null && (!items.length || messages.some((item) => sequenceOf(item) === held))) return held;
  if (!unreadCount) return null;
  return messages.reduce((oldest, item) => {
    const sequence = sequenceOf(item);
    if (!Number.isFinite(sequence) || sequence <= cursor) return oldest;
    return oldest === null ? sequence : Math.min(oldest, sequence);
  }, null);
}

export const UNREAD_LINE_GRACE_MS = 60_000;

const readObservation = (reading, previousRead) => {
  const messages = agentMessages(reading.items);
  const cursor = reading.cursor;
  const pending = messages.filter((item) => sequenceOf(item) > cursor);
  // A read tail is not enough when older unread messages are outside the page.
  const windowCoversUnread = pending.length >= (reading.unreadCount || 0);
  return {
    newest: Math.max(0, ...messages.map(sequenceOf)),
    windowCoversUnread,
    readThrough: Math.max(previousRead, Number.isFinite(cursor) ? cursor : 0,
      windowCoversUnread ? reading.readThrough || 0 : 0),
  };
};

// One visit's marker. Read observations are retained across paints so a stale
// digest cannot restore a line whose grace period has already ended.
export function createUnreadMarker(onExpire) {
  let held = null;
  let timer = null;
  let readThrough = 0;
  let latestRead = 0;
  const cancel = () => {
    clearTimeout(timer);
    timer = null;
  };
  return {
    update(reading) {
      const observation = readObservation(reading, readThrough);
      const { newest, windowCoversUnread } = observation;
      readThrough = observation.readThrough;
      held = unreadAnchorSequence({ ...reading, cursor: Math.max(reading.cursor, latestRead), held });
      const allRead = windowCoversUnread && newest > 0 && readThrough >= newest;
      if (!allRead) cancel();
      if (held !== null && allRead && (timer === null || newest > latestRead)) {
        cancel();
        latestRead = Math.max(latestRead, newest);
        timer = setTimeout(() => {
          timer = null;
          held = null;
          onExpire();
        }, UNREAD_LINE_GRACE_MS);
      }
      return held;
    },
    leave() {
      cancel();
      held = null;
    },
  };
}
