/** How near the end still counts as reading the end. Absorbs the fractional
 *  scroll heights a zoomed or sub-pixel layout leaves behind. */
const AT_BOTTOM_SLACK_PX = 32;

/** How much of what came before the unread line is left showing above it. A
 *  line landing flush on the top edge reads as the top of the world; a sliver
 *  of the conversation above it reads as the boundary it is. */
const LINE_HEADROOM_PX = 12;

/// How long after the last scroll event a list is still taken to be moving
/// under the reader's finger or a fling, and how long after the last touch or
/// wheel a run of scroll events is still the reader's rather than a paint's.
const SCROLL_SETTLE_MS = 200;
const FLING_MS = 3000;

/// What each scroller knows about the reader moving it. Wired once per
/// scroller by `wireReaderMotion`; a scroller nobody wired is never moving.
const readerMotion = new WeakMap();
const now = () => (typeof performance !== "undefined" && performance.now ? performance.now() : Date.now());

/// Listen for the reader moving `scroller`: a finger on it, a wheel, and the
/// scroll events that follow either. Idempotent.
///
/// On iOS, writing scrollTop during a fling ends the fling and lands the reader
/// where the write said — so a paint that "keeps the reader's place" while they
/// are flinging keeps sending them back to it. The policies below ask here
/// before writing, and write nothing while the list is moving under the reader.
export function wireReaderMotion(scroller) {
  if (!scroller || readerMotion.has(scroller)) return;
  const state = { touching: false, lastScrollAt: 0, lastInputAt: -Infinity };
  readerMotion.set(scroller, state);
  const passive = { passive: true };
  const input = () => {
    state.lastInputAt = now();
  };
  scroller.addEventListener("touchstart", () => {
    state.touching = true;
    input();
  }, passive);
  for (const type of ["touchend", "touchcancel"]) {
    scroller.addEventListener(type, () => {
      state.touching = false;
      input();
    }, passive);
  }
  scroller.addEventListener("wheel", input, passive);
  scroller.addEventListener("scroll", () => {
    state.lastScrollAt = now();
  }, passive);
}

/// Whether the reader is moving `scroller` right now: a finger on it, or
/// scroll events still arriving shortly after a touch or a wheel.
export function readerIsMoving(scroller) {
  const state = readerMotion.get(scroller);
  if (!state) return false;
  if (state.touching) return true;
  const at = now();
  return at - state.lastScrollAt < SCROLL_SETTLE_MS && at - state.lastInputAt < FLING_MS;
}

/// Put `scroller` at `top`, unless it is already there. A write that changes
/// nothing is not free: on iOS it ends a fling in progress.
export function writeScrollTop(scroller, top) {
  if (Math.abs(scroller.scrollTop - top) < 1) return;
  scroller.scrollTop = top;
}

/// Run `paint` and report whether it moved anything under `scroller`.
///
/// Asking the DOM is the only honest answer: the paint belongs to the caller,
/// and a poll's repaint that resolved the same conversation writes nothing at
/// all. Observing it costs one observer per tick and tells the difference
/// between a repaint and a tick that merely happened.
export function paintAndSayWhetherAnythingMoved(scroller, paint) {
  if (typeof MutationObserver !== "function") {
    paint();
    return true;
  }
  const observer = new MutationObserver(() => {});
  observer.observe(scroller, { childList: true, subtree: true, attributes: true, characterData: true });
  try {
    paint();
    return observer.takeRecords().length > 0;
  } finally {
    observer.disconnect();
  }
}

/// Whether the reader is standing at the end of `scroller`.
///
/// The one reading of it, so the paint that decides where to land and the panel
/// that decides whether the reader has caught up can never disagree.
export const isAtBottom = (scroller) =>
  !!scroller && scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <= AT_BOTTOM_SLACK_PX;

export function paintKeepingPlace(scroller, paint, { opening, policy }) {
  if (!scroller) {
    paint();
    return;
  }
  const isOpening = opening(scroller);
  const held = policy.hold(scroller, isOpening);
  const changed = paintAndSayWhetherAnythingMoved(scroller, paint);
  policy.restore(scroller, held, changed);
}

const READING_CONTENT = "p, pre, li, blockquote";
const readingEntries = (scroller) => [...scroller.querySelectorAll("[data-key]:not(button)")];

/** The visible reading area starts below a header pinned over the content. */
function readingViewport(scroller) {
  const viewport = scroller.getBoundingClientRect();
  const top = [...scroller.querySelectorAll("header")].reduce((edge, header) => {
    const box = header.getBoundingClientRect();
    const pinned = ["sticky", "fixed"].includes(getComputedStyle(header).position);
    return pinned && box.top <= edge && box.bottom > edge ? box.bottom : edge;
  }, viewport.top);
  return { top, bottom: viewport.bottom };
}

const intersectsViewport = (element, viewport) => {
  const box = element.getBoundingClientRect();
  return box.top < viewport.bottom && box.bottom > viewport.top;
};

/** Keep a visible content node, including one inside a very tall entry.
 *  Buttons with keys are navigation ticks, rather than timeline content. */
function holdReadingAnchor(scroller) {
  const viewport = readingViewport(scroller);
  const row = readingEntries(scroller)
    .find((element) => intersectsViewport(element, viewport));
  if (!row) return null;
  const contents = [...row.querySelectorAll(READING_CONTENT)];
  const element = contents.find((node) => intersectsViewport(node, viewport)) || row;
  const top = scroller.getBoundingClientRect().top;
  return {
    row, element, key: row.getAttribute("data-key"), index: contents.indexOf(element), tag: element.tagName,
    offset: element.getBoundingClientRect().top - top,
    rowOffset: row.getBoundingClientRect().top - top,
  };
}

function restoreReadingAnchor(scroller, held) {
  const row = scroller.contains(held.row) ? held.row
    : readingEntries(scroller).find((element) => element.getAttribute("data-key") === held.key);
  if (!row) return;
  const candidate = row.contains(held.element) ? held.element : row.querySelectorAll(READING_CONTENT)[held.index];
  const element = candidate?.tagName === held.tag ? candidate : row;
  const offset = element === row ? held.rowOffset : held.offset;
  const moved = element.getBoundingClientRect().top - scroller.getBoundingClientRect().top - offset;
  // Native anchoring may already have moved the scroller. Correct only the
  // displacement that remains, rather than adding it to an old scrollTop.
  writeScrollTop(scroller, scroller.scrollTop + moved);
}

/// Where the reader belongs in a conversation that is still being written.
///
/// A reader who is at the end is FOLLOWING, and a paint keeps them at the end:
/// what has just arrived is drawn under what they were reading, and the end is
/// where they were. A reader who has scrolled up is reading, and nothing moves
/// them. Neither is moved while the list is moving under their finger or a
/// fling (`readerIsMoving`): a write then only ends the fling where it stood.
///
/// The one time the line is the landing spot is when the panel OPENS: the
/// reader came for the first thing they have not read, not the last thing
/// said, and reading a burst starts at its beginning. `unreadSelector` names
/// the line (core/thread.js rules it); without one the end of the conversation
/// is the only place to land.
///
/// `olderItemsPrepended` says this paint grew the timeline at the TOP — a page
/// of history the reader asked for by scrolling back past the start of the
/// window. Everything they were reading has moved down by the height of what
/// arrived, so keeping their scrollTop would keep the pixel and lose the
/// message, jumping them a page further back on every load.
export function followConversation({ olderItemsPrepended = false, unreadSelector = null } = {}) {
  /// The scrollTop that puts the top of the unread line at the top of the
  /// viewport, or the end of the conversation when there is no line to land on.
  /// Never past the end: a line in the last screenful cannot reach the top, and
  /// asking for more than there is would only show the end anyway.
  const landing = (scroller) => {
    const line = unreadSelector && scroller.querySelector(unreadSelector);
    if (!line) return scroller.scrollHeight;
    const above = line.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    const wanted = scroller.scrollTop + above - LINE_HEADROOM_PX;
    return Math.max(0, Math.min(wanted, scroller.scrollHeight - scroller.clientHeight));
  };
  return {
    hold: (scroller, opening) => ({
      opening,
      scrollTop: scroller.scrollTop,
      scrollHeight: scroller.scrollHeight,
      atBottom: isAtBottom(scroller),
      anchor: opening ? null : holdReadingAnchor(scroller),
    }),
    restore: (scroller, held, changed) => {
      if (!held.opening && !changed) return;
      if (olderItemsPrepended) {
        writeScrollTop(scroller, held.scrollTop + (scroller.scrollHeight - held.scrollHeight));
        return;
      }
      if (!held.opening && readerIsMoving(scroller)) return;
      if (!held.opening) {
        if (held.atBottom) writeScrollTop(scroller, scroller.scrollHeight);
        else if (held.anchor) restoreReadingAnchor(scroller, held.anchor);
        else writeScrollTop(scroller, held.scrollTop);
        return;
      }
      const land = () => {
        writeScrollTop(scroller, landing(scroller));
      };
      land();
      if (typeof requestAnimationFrame === "function") requestAnimationFrame(land);
    },
  };
}

export function anchorTop(selector = "[data-key]") {
  const keyedElements = (scroller) => [...scroller.querySelectorAll(selector)];
  const withKey = (scroller, key) =>
    keyedElements(scroller).find((element) => element.getAttribute("data-key") === key) || null;
  const offsetIn = (scroller, element) =>
    element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
  return {
    hold: (scroller, opening) => {
      if (opening) return null;
      const viewport = scroller.getBoundingClientRect();
      for (const element of keyedElements(scroller)) {
        const box = element.getBoundingClientRect();
        if (box.bottom <= viewport.top || box.top >= viewport.bottom) continue;
        return { key: element.getAttribute("data-key"), offset: box.top - viewport.top };
      }
      return null;
    },
    restore: (scroller, held, changed) => {
      if (!held || !changed) return;
      const standing = withKey(scroller, held.key);
      if (!standing) return;
      const moved = offsetIn(scroller, standing) - held.offset;
      if (moved) writeScrollTop(scroller, scroller.scrollTop + moved);
    },
  };
}
