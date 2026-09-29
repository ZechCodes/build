// Whether a task notice is one line or stacked (#217).
//
// A notice is one line when the whole of it fits, and when it does not it
// stacks all at once: the action and its task on the first line, each name
// indented on a line of its own (styles/tasks.css). Never a partial wrap — and
// whether the whole of it fits depends on its words, which no CSS rule can
// ask. So it is measured: drawn as one line, and stacked if that line
// overflows.
//
// Long conversations hold hundreds of notices and the rail repaints on a
// poll, so a notice is measured again only when its words or the scroller's
// width are not what it was fitted to. What it was fitted to lives here, off
// the DOM; the one mark on the DOM is `data-fit`, which a repaint leaves
// alone (core/domPatch.js) and which is written only when it changes. Every
// fit is three batched passes (unstack, read, restack): one forced layout,
// not one per notice.

import { LAYOUT_FIT_ATTRIBUTE } from "./domPatch.js";

const NOTICES = ".thread-task-notice[data-task-notice]";
const STACKED = "stacked";

/** What each notice was last fitted to. A font landing moves every line's
 *  width without changing a word or the scroller, so it starts a new
 *  generation of keys. */
const fittedTo = new WeakMap();
let generation = 0;
const fitKey = (notice, width) => `${generation}:${width}:${notice.textContent}`;

const isStacked = (notice) => notice.getAttribute(LAYOUT_FIT_ATTRIBUTE) === STACKED;

/** Fit the notices in `scroller` whose words or width moved since their last fit. */
export function fitTaskNotices(scroller) {
  const width = scroller?.clientWidth || 0;
  if (!width) return;
  const unfitted = [...scroller.querySelectorAll(NOTICES)]
    .map((notice) => ({ notice, key: fitKey(notice, width), was: isStacked(notice) }))
    .filter(({ notice, key }) => fittedTo.get(notice) !== key);
  for (const { notice, was } of unfitted) if (was) notice.removeAttribute(LAYOUT_FIT_ATTRIBUTE);
  const overflowing = unfitted.filter(({ notice }) => notice.scrollWidth > notice.clientWidth);
  for (const { notice, key } of unfitted) fittedTo.set(notice, key);
  for (const { notice } of overflowing) notice.setAttribute(LAYOUT_FIT_ATTRIBUTE, STACKED);
}

const watched = new WeakSet();

/** Refit whenever the scroller's width changes; a height-only change (the
 *  conversation growing, the composer resizing) leaves every notice as it
 *  was. Once per scroller, however often it is painted. */
export function keepTaskNoticesFitted(scroller) {
  if (!scroller || watched.has(scroller) || typeof ResizeObserver === "undefined") return;
  watched.add(scroller);
  let width = scroller.clientWidth;
  new ResizeObserver(() => {
    if (scroller.clientWidth === width) return;
    width = scroller.clientWidth;
    fitTaskNotices(scroller);
  }).observe(scroller);
  scroller.ownerDocument?.fonts?.ready.then(() => {
    generation += 1;
    fitTaskNotices(scroller);
  });
}
