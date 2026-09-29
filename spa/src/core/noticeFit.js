// Whether a task notice is one line or stacked (#217).
//
// A notice is one line when the whole of it fits, and when it does not it
// stacks all at once: the action on the first line, each section indented on
// a line of its own (styles/tasks.css). Never a partial wrap — and whether the
// whole of it fits depends on its words, which no CSS rule can ask. So it is
// measured: drawn as one line, and stacked if that line overflows.
//
// Long conversations hold hundreds of notices, so a notice remembers the
// width it was fitted at and is measured again only when that width is not
// the scroller's any more — a new row, or a resize. Every fit is three
// batched passes (unstack, read, restack): one forced layout, not one per
// notice.

const NOTICES = ".thread-task-notice[data-task-notice]";
export const STACKED = "is-stacked";

/** Fit the notices in `scroller` that were not fitted at its width. */
export function fitTaskNotices(scroller) {
  const width = String(scroller?.clientWidth || 0);
  if (width === "0") return;
  const unfitted = [...scroller.querySelectorAll(NOTICES)].filter((notice) => notice.dataset.fitWidth !== width);
  for (const notice of unfitted) notice.classList.remove(STACKED);
  const overflowing = unfitted.filter((notice) => notice.scrollWidth > notice.clientWidth);
  for (const notice of unfitted) notice.dataset.fitWidth = width;
  for (const notice of overflowing) notice.classList.add(STACKED);
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
  // A web font landing changes every line's width without resizing anything.
  scroller.ownerDocument?.fonts?.ready.then(() => {
    for (const notice of scroller.querySelectorAll(NOTICES)) delete notice.dataset.fitWidth;
    fitTaskNotices(scroller);
  });
}
