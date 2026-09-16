import { isAtBottom } from "./paintKeepingPlace.js";

const COMPOSER_CLEARANCE_PROPERTY = "--rail-composer-clearance";

/** Reserve a floating composer's height without moving someone reading back. */
export function mountComposerClearance(panel) {
  const scroller = panel?.querySelector("#rail-body");
  const composer = panel?.querySelector("#rail-composer");
  if (!scroller || !composer) return () => {};

  const sync = () => {
    const wasAtBottom = isAtBottom(scroller);
    const heldScrollTop = scroller.scrollTop;
    const height = composer.getBoundingClientRect().height;
    scroller.style.setProperty(COMPOSER_CLEARANCE_PROPERTY, `${height}px`);
    scroller.scrollTop = wasAtBottom ? scroller.scrollHeight : heldScrollTop;
  };
  const observer = typeof ResizeObserver === "function" ? new ResizeObserver(sync) : null;
  sync();
  observer?.observe(composer);
  return () => {
    observer?.disconnect();
    scroller.style.removeProperty(COMPOSER_CLEARANCE_PROPERTY);
  };
}
