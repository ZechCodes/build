/** How near the end still counts as reading the end. Absorbs the fractional
 *  scroll heights a zoomed or sub-pixel layout leaves behind. */
const AT_BOTTOM_SLACK_PX = 32;

/** How much of what came before the unread line is left showing above it. A
 *  line landing flush on the top edge reads as the top of the world; a sliver
 *  of the conversation above it reads as the boundary it is. */
const LINE_HEADROOM_PX = 12;

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

/// Where the reader belongs in a conversation that is still being written.
///
/// A reader who is at the end is FOLLOWING, and a paint puts them where the
/// reading resumes: on the unread line if the timeline carries one, and at the
/// newest message if it does not. Landing on the line rather than the bottom is
/// what makes a burst of messages readable — the reader arrives at the
/// beginning of what is new instead of its end. A reader who has scrolled up is
/// reading, and nothing moves them.
///
/// `unreadSelector` names the line (core/thread.js rules it); without one the
/// end of the conversation is the only place to land.
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
    }),
    restore: (scroller, held, changed) => {
      if (!held.opening && !changed) return;
      if (olderItemsPrepended) {
        scroller.scrollTop = held.scrollTop + (scroller.scrollHeight - held.scrollHeight);
        return;
      }
      if (!held.opening && !held.atBottom) {
        scroller.scrollTop = held.scrollTop;
        return;
      }
      const land = () => {
        scroller.scrollTop = landing(scroller);
      };
      land();
      const layoutMaySettleLate = held.opening && typeof requestAnimationFrame === "function";
      if (layoutMaySettleLate) requestAnimationFrame(land);
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
      if (moved) scroller.scrollTop += moved;
    },
  };
}
