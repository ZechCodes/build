// Keeping the reader's place across a paint.
//
// Every surface here re-renders on a poll, and a repaint is the same lever as
// the open: whatever writes the content also decides where the reader ends up.
// So the two are one call — the paint, and the policy that says where the
// reader is left. A conversation pins its newest message to the bottom; a diff
// holds the file being read still while the stack grows above it.
//
// The one rule both policies keep: a tick whose paint wrote nothing is not a
// repaint, and the scroller is not touched for it — not even to write back the
// number it already holds. That assignment is not free: on iOS it cancels the
// momentum of a flick in progress and drops the reader back where the tick
// found them, which at a poll every 1.6 seconds is a surface that cannot be
// scrolled at all.

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

/// Paint into `scroller`, keeping the reader where `policy` says they belong.
///
/// `opening(scroller)` says whether this paint is the surface's open — the tab
/// was just selected, or a shell rebuild wiped the body under it — which is
/// the one paint that may move the reader without their asking. With no
/// scroller this is `paint()` and nothing else.
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

/// The newest message is the one the human came for and it sits at the END, so
/// a conversation opens at the bottom, a reader already there is carried along
/// with what arrives, and a reader who scrolled up is left exactly where they
/// were rather than yanked back down mid-sentence.
///
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
      if (olderItemsPrepended) {
        scroller.scrollTop = held.scrollTop + (scroller.scrollHeight - held.scrollHeight);
        return;
      }
      if (!held.opening && !changed) return;
      if (!held.opening && !held.atBottom) {
        scroller.scrollTop = held.scrollTop;
        return;
      }
      const toBottom = () => {
        scroller.scrollTop = scroller.scrollHeight;
      };
      toBottom();
      // Markdown and web fonts can settle a frame after the content lands,
      // leaving the open short of the newest message. Only the open re-pins:
      // doing it on a poll's repaint would fight a reader who scrolled away
      // within that frame.
      if (held.opening && typeof requestAnimationFrame === "function") requestAnimationFrame(toBottom);
    },
  };
}

/// A stack is read from the top down, so the thing to hold still is the block
/// the reader's eye is on: the topmost keyed element the viewport touches. Its
/// offset from the top of the viewport is recorded before the paint and
/// restored after, so a file that grew above it — an agent writing into it
/// while the human reads further down — moves the stack, not the reader.
export function anchorTop(selector = "[data-key]") {
  const keyedElements = (scroller) => [...scroller.querySelectorAll(selector)];
  const withKey = (scroller, key) =>
    keyedElements(scroller).find((element) => element.getAttribute("data-key") === key) || null;
  const offsetIn = (scroller, element) =>
    element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
  return {
    hold: (scroller, opening) => {
      if (opening) return null; // nothing is standing there yet to hold
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
      if (!standing) return; // the block it named left the stack — nothing to hold
      const moved = offsetIn(scroller, standing) - held.offset;
      if (moved) scroller.scrollTop += moved;
    },
  };
}
