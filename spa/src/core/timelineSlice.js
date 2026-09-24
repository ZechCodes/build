// How much of a held conversation the panel draws.
//
// The record holds everything the reader ever widened into, which on a busy
// agent is thousands of items, and drawing all of it on open laid out every one
// of them inside the panel's opening animation (#158). So the panel draws the
// newest entries of what it holds and a row above them that shows more; the
// rest stays in the cache until it is asked for.
//
// The slice is measured in entries, the timeline's own rows: a message is one,
// and a folded run of activity is one however many calls it holds. What the
// panel remembers between paints is a FLOOR, a sequence, rather than a count,
// so a push that lands at the bottom grows the slice instead of pushing its top
// row out: nothing a repaint does ever shrinks it.

/** How many entries a first paint draws, and how many more each ask shows. */
export const TIMELINE_SLICE_SIZE = 60;

/** The key of the row above a cut slice. */
export const EARLIER_ENTRY_KEY = "earlier";

/// The row that shows more: drawn like the Git pane's "Load older commits…"
/// row, and a button because it is one.
export const EARLIER_ENTRY = {
  key: EARLIER_ENTRY_KEY,
  html: '<button type="button" class="thread-earlier">Show earlier messages</button>',
};

/// Where a slice reaching down to `floor` starts: just after the newest entry
/// that ends below it. An entry reaching the floor is kept whole, so a run that
/// older history extended upwards is still one kept row.
function cutAt(entries, floor) {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    if (entries[index].through < floor) return index + 1;
  }
  return 0;
}

/// The floor a kept slice stands on: its oldest sequence. None when nothing it
/// kept carries one, which is a conversation drawn without sequences.
function floorOf(kept) {
  const first = kept.find((entry) => Number.isFinite(entry.from));
  return first ? first.from : null;
}

/**
 * The entries of `entries` (oldest first, each with numeric `from`/`through`)
 * the panel draws.
 *
 * - `floor`: the sequence the last paint's slice started at. None draws the
 *   newest `size`, which is a first paint.
 * - `reach`: a sequence the slice has to reach down to whatever the floor says,
 *   so a target above it (the unread line, a deep link) is drawn to be landed
 *   on.
 * - `extend`: how many more entries above the floor to show, the reader having
 *   asked for them.
 *
 * Answers the kept entries, how many were left above them, and the floor to
 * remember for the next paint.
 */
export function sliceTimeline(entries, { floor = null, reach = null, extend = 0, size = TIMELINE_SLICE_SIZE } = {}) {
  const reached = Number.isFinite(reach) ? cutAt(entries, reach) : entries.length;
  const cut = Math.max(0, Math.min(cutAtFloor(entries, floor, size), reached) - extend);
  const kept = entries.slice(cut);
  return { entries: kept, hidden: cut, floor: floorOf(kept) ?? floor };
}

/// Where the slice standing on `floor` starts, or the newest `size` entries on
/// a first paint. A floor above everything held is a conversation that is not
/// the one it was measured on any more, and is drawn as one being opened.
function cutAtFloor(entries, floor, size) {
  const newest = Math.max(0, entries.length - size);
  if (!Number.isFinite(floor)) return newest;
  const cut = cutAt(entries, floor);
  return cut < entries.length ? cut : newest;
}

/**
 * What the panel remembers of one conversation's slice between paints.
 *
 * `request()` is what the next paint slices with, and `settle(sliced)` is how
 * the paint says what it drew. `showEarlier()` asks the next paint for another
 * `size` entries above the floor, and `reachDown(sequence)` for everything down
 * to a sequence. `reset()` forgets it all, for a different conversation.
 */
export function createTimelineSlice({ size = TIMELINE_SLICE_SIZE } = {}) {
  let floor = null;
  let extend = 0;
  let reach = null;
  let hidden = 0;
  return {
    request: () => ({ floor, extend, reach, size }),
    settle(sliced) {
      floor = sliced.floor;
      hidden = sliced.hidden;
      extend = 0;
      reach = null;
    },
    /** Whether the held conversation has entries above what is drawn. */
    hasHiddenEntries: () => hidden > 0,
    /** Where what is drawn starts, while that is not where the held
     *  conversation starts; null while all of it is drawn. */
    drawnFloor: () => (hidden > 0 ? floor : null),
    showEarlier() {
      extend += size;
    },
    reachDown(sequence) {
      const wanted = Number(sequence);
      if (!Number.isFinite(wanted)) return;
      reach = Number.isFinite(reach) ? Math.min(reach, wanted) : wanted;
    },
    /** What the next paint has to draw differently, as a string the paint's
     *  fingerprint can carry. */
    signature: () => `${floor ?? ""}:${extend}:${reach ?? ""}`,
    reset() {
      floor = null;
      extend = 0;
      reach = null;
      hidden = 0;
    },
  };
}
