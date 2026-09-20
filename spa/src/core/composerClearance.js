import { isAtBottom, readerIsMoving, writeScrollTop } from "./paintKeepingPlace.js";

const COMPOSER_CLEARANCE_PROPERTY = "--rail-composer-clearance";

/** Reserve a floating composer's height without moving someone reading back. */
export function mountComposerClearance(panel) {
  const scroller = panel?.querySelector("#rail-body");
  const composer = panel?.querySelector("#rail-composer");
  if (!scroller || !composer) return () => {};

  const sync = () => {
    const wasAtBottom = isAtBottom(scroller);
    const height = composer.getBoundingClientRect().height;
    scroller.style.setProperty(COMPOSER_CLEARANCE_PROPERTY, `${height}px`);
    // A reader in history is left exactly where they are: the clearance grows
    // under the list, not above their place. One at the bottom follows it —
    // unless the list is moving under them, when a write would only stop it.
    if (wasAtBottom && !readerIsMoving(scroller)) writeScrollTop(scroller, scroller.scrollHeight);
  };
  const observer = typeof ResizeObserver === "function" ? new ResizeObserver(sync) : null;
  sync();
  observer?.observe(composer);
  return () => {
    observer?.disconnect();
    scroller.style.removeProperty(COMPOSER_CLEARANCE_PROPERTY);
  };
}
