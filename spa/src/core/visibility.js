/** Whether the page is on screen right now. The gate every poll — and every
 *  pushed change event — consults, so both answer the same question the same
 *  way. */
export function pageVisible() {
  return typeof document === "undefined" || !document.hidden;
}

/** Gate a poll callback on page visibility: a hidden tab's timers keep firing
 *  (browser-throttled), and every firing drives the bridge's expensive diff
 *  work — so hidden tabs skip the call entirely and the next visible tick
 *  catches up. Wrap the interval CALLBACK, never the direct call sites, so
 *  mounting and user actions always go through. */
export function whenVisible(fn) {
  return (...args) => (pageVisible() ? fn(...args) : undefined);
}
