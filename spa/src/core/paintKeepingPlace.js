/** How near the end still counts as reading the end. Absorbs the fractional
 *  scroll heights a zoomed or sub-pixel layout leaves behind. */
const AT_BOTTOM_SLACK_PX = 32;

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

/// `olderItemsPrepended` says this paint grew the timeline at the TOP — a page
/// of history the reader asked for by scrolling back past the start of the
/// window. Everything they were reading has moved down by the height of what
/// arrived, so keeping their scrollTop would keep the pixel and lose the
/// message, jumping them a page further back on every load.
export function pinToBottom({ olderItemsPrepended = false } = {}) {
  return {
    hold: (scroller, opening) => ({
      opening,
      scrollTop: scroller.scrollTop,
      scrollHeight: scroller.scrollHeight,
      atBottom: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <= AT_BOTTOM_SLACK_PX,
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
      const pinToNewest = () => {
        scroller.scrollTop = scroller.scrollHeight;
      };
      pinToNewest();
      const layoutMaySettleLate = held.opening && typeof requestAnimationFrame === "function";
      if (layoutMaySettleLate) requestAnimationFrame(pinToNewest);
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
