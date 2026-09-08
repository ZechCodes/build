// Where the reader's unread begins.
//
// The daemon holds a read cursor per conversation and says how much is waiting
// past it. This is the one place those two facts become a PLACE in the
// timeline: the sequence the divider is ruled above, and the message a repaint
// opens the conversation on.
//
// Both readings come from here so they can never disagree — a line ruled in one
// place while the scroll lands in another is worse than neither.

const sequenceOf = (item) => Number((item && item.data && item.data.sequence) ?? NaN);

/// The oldest sequence the window holds past `cursor`, or null when everything
/// in it has been read.
const firstSequenceAfter = (items, cursor) =>
  items.reduce((oldest, item) => {
    const sequence = sequenceOf(item);
    if (!Number.isFinite(sequence) || sequence <= cursor) return oldest;
    return oldest === null ? sequence : Math.min(oldest, sequence);
  }, null);

const windowHolds = (items, sequence) => items.some((item) => sequenceOf(item) === sequence);

/// Whether the line has done its job: the reader is at the end of what the
/// panel holds and nothing is waiting past the cursor.
///
/// Both halves are needed, because the bottom of a WINDOW is not the end of the
/// conversation: a message waiting under the tail is still waiting.
const nothingLeftToMark = ({ caughtUp, unreadCount }) => !!caughtUp && !unreadCount;

/// The line this conversation already has, for as long as the window it stands
/// in still holds the message it was ruled above.
///
/// This is what keeps the line still. A message is read the moment its bottom
/// edge comes into view, so a line computed from the live cursor alone would
/// rule itself above what just arrived, watch the cursor pass it, and vanish
/// inside a second. A paint with nothing loaded is not evidence of anything, so
/// it keeps the line too.
const lineStillStanding = ({ held, items }) => {
  if (held === null || held === undefined) return null;
  if (!items || !items.length) return held;
  return windowHolds(items, held) ? held : null;
};

/// Where the unread line stands: the sequence it is ruled above, or null over a
/// conversation the reader has nothing left to mark in.
export function unreadAnchorSequence(reading = {}) {
  if (nothingLeftToMark(reading)) return null;
  const standing = lineStillStanding(reading);
  if (standing !== null) return standing;
  if (!reading.unreadCount) return null;
  return firstSequenceAfter(reading.items || [], reading.cursor || 0);
}
