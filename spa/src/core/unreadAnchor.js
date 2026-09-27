// Chat uses numbered agent replies; task timelines use ULIDs and count any
// entry by somebody other than the reader. Both share the hold and grace rules.
const sequenceOf = (item) => Number(item?.data?.sequence ?? NaN);
export const isAgentMessage = (item) => item?.type === "message" && item.data?.role === "agent";
const chatItems = {
  eligible: isAgentMessage,
  key: sequenceOf,
  valid: Number.isFinite,
  compare: (left, right) => left - right,
  first: 0,
};
const later = (rules, left, right) => rules.compare(left, right) > 0;
const maximum = (rules, left, right) => later(rules, left, right) ? left : right;
const messagesOf = (rules, items) => items.filter((item) => rules.eligible(item) && rules.valid(rules.key(item)));

export function unreadAnchorSequence({ cursor, held, items = [], unreadCount } = {}, rules = chatItems) {
  if (!rules.valid(cursor)) return null;
  const messages = messagesOf(rules, items);
  if (held != null && (!items.length || messages.some((item) => rules.compare(rules.key(item), held) === 0))) return held;
  if (!unreadCount) return null;
  return messages.reduce((oldest, item) => {
    const key = rules.key(item);
    if (!later(rules, key, cursor)) return oldest;
    return oldest === null || later(rules, oldest, key) ? key : oldest;
  }, null);
}

export const UNREAD_LINE_GRACE_MS = 60_000;

const readObservation = (reading, previousRead, rules) => {
  const messages = messagesOf(rules, reading.items);
  const pending = messages.filter((item) => later(rules, rules.key(item), reading.cursor));
  // A read tail is not enough when older unread messages are outside the page.
  const windowCoversUnread = pending.length >= (reading.unreadCount || 0);
  const newest = messages.reduce((key, item) => maximum(rules, key, rules.key(item)), rules.first);
  const cursor = rules.valid(reading.cursor) ? reading.cursor : rules.first;
  const readThrough = rules.valid(reading.readThrough) ? reading.readThrough : rules.first;
  return {
    newest,
    windowCoversUnread,
    readThrough: maximum(rules, previousRead, maximum(rules, cursor, windowCoversUnread ? readThrough : rules.first)),
  };
};

// One visit's marker. Read observations are retained across paints so a stale
// digest cannot restore a line whose grace period has already ended.
export function createUnreadMarker(onExpire, rules = chatItems) {
  let held = null;
  let timer = null;
  let readThrough = rules.first;
  let latestRead = rules.first;
  const cancel = () => {
    clearTimeout(timer);
    timer = null;
  };
  return {
    update(reading) {
      const observation = readObservation(reading, readThrough, rules);
      const { newest, windowCoversUnread } = observation;
      readThrough = observation.readThrough;
      const cursor = rules.valid(reading.cursor) ? maximum(rules, reading.cursor, latestRead) : reading.cursor;
      held = unreadAnchorSequence({ ...reading, cursor, held }, rules);
      const allRead = windowCoversUnread && later(rules, newest, rules.first) && !later(rules, newest, readThrough);
      if (!allRead) cancel();
      if (held !== null && allRead && (timer === null || later(rules, newest, latestRead))) {
        cancel();
        latestRead = maximum(rules, latestRead, newest);
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
